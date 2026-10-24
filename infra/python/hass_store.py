"""Fence store publication with a durable Git state ref and atomic, leased updates."""

import json
import os
import re
import subprocess
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from json_data import Object, json_bytes, object_value, read_json, text

FENCE = 'hass-publication'
STATE_FILE = 'publication.json'
STATE_SUBJECTS = {
    'prepared': 'Prepare HASS publication',
    'advertised': 'Advertise HASS release',
    'qualified': 'Qualify HASS release',
    'aborted': 'Abort HASS publication',
    'withdrawn': 'Withdraw HASS release',
}


def oid(value: object, *, empty: bool = False) -> str:
    if not isinstance(value, str) or re.fullmatch(r'(?:[0-9a-f]{40})?' if empty else r'[0-9a-f]{40}', value) is None:
        raise ValueError('Invalid store promotion object ID')
    return value


class Git(Protocol):
    def __call__(self, *arguments: str, cwd: Path | None = None) -> str: ...


def write_journal(directory: Path, data: Object, *, replace: bool = False, name: str = 'promotion.json') -> None:
    with tempfile.TemporaryDirectory(prefix='.promotion.', dir=directory) as temporary:
        staged, journal = Path(temporary) / name, directory / name
        with staged.open('xb') as stream:
            stream.write(json_bytes(data))
            stream.flush()
            os.fsync(stream.fileno())
        if replace:
            os.replace(staged, journal)
        else:
            os.link(staged, journal)
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


@dataclass(frozen=True)
class Store:
    remote: str
    git: Git
    head: Callable[[str], str]
    branch: str = 'hass-apps'
    fence: str = FENCE

    def fetch(self, checkout: Path, commit: str) -> None:
        if commit:
            self.git('fetch', '--no-tags', '--depth=2', self.remote, commit, cwd=checkout)

    def state(self, checkout: Path, commit: str, *, fetch: bool = True) -> Object:
        if fetch:
            self.fetch(checkout, commit)
        tree = self.git('ls-tree', commit, cwd=checkout)
        if re.fullmatch(r'100644 blob [0-9a-f]{40}\tpublication\.json', tree):
            payload = self.git('show', commit + ':' + STATE_FILE, cwd=checkout)
        elif not tree:
            payload = self.git('show', '-s', '--format=%B', commit, cwd=checkout)
        else:
            raise ValueError('Expected publication.json or a legacy empty publication tree')
        data = object_value(json.loads(payload))
        if text(data.get('status')) not in STATE_SUBJECTS:
            raise ValueError('Invalid durable store promotion state')
        oid(data['previous'], empty=True)
        oid(data['published'])
        return data

    def commit(self, checkout: Path, data: Object, parent: str, *, subject: str | None = None) -> str:
        status = text(data.get('status'))
        if status not in STATE_SUBJECTS:
            raise ValueError('Invalid durable store promotion state')
        oid(data['previous'], empty=True)
        oid(data['published'])
        with tempfile.TemporaryDirectory(prefix='.hass-state.', dir=checkout) as temporary:
            path = Path(temporary).resolve() / STATE_FILE
            path.write_bytes(json_bytes(data))
            blob = oid(self.git('hash-object', '-w', str(path), cwd=checkout))
            # A single regular-file tree avoids changing the metadata checkout's index.
            tree_path = path.parent / 'tree'
            tree_path.write_bytes(b'100644 ' + STATE_FILE.encode() + b'\0' + bytes.fromhex(blob))
            tree = self.git('hash-object', '-t', 'tree', '-w', str(tree_path), cwd=checkout)
        return self.commit_tree(checkout, tree, parent, subject or STATE_SUBJECTS[status])

    def commit_tree(self, checkout: Path, tree: str, parent: str, subject: str) -> str:
        oid(tree)
        oid(parent, empty=True)
        return self.git(
            '-c',
            'user.name=Garmin release',
            '-c',
            'user.email=release@users.noreply.github.com',
            'commit-tree',
            tree,
            *(['-p', parent] if parent else []),
            '-m',
            subject,
            cwd=checkout,
        )

    def push(self, checkout: Path, before: str, after: str, *, advertisement: tuple[str, str] | None = None) -> None:
        self.git(
            'push',
            '--atomic',
            '--force-with-lease=refs/heads/' + self.fence + ':' + before,
            *(['--force-with-lease=refs/heads/' + self.branch + ':' + advertisement[0]] if advertisement else []),
            self.remote,
            after + ':refs/heads/' + self.fence,
            *([advertisement[1] + ':refs/heads/' + self.branch] if advertisement else []),
            cwd=checkout,
        )

    def prepare(self, checkout: Path, directory: Path, previous: str, published: str) -> tuple[str, str] | None:
        oid(previous, empty=True)
        oid(published)
        before = self.head(self.fence)
        state = self.state(checkout, before) if before else {}
        if previous == published:
            if state.get('status') != 'qualified' or state.get('published') != published:
                raise ValueError('Inherited store metadata has no durable qualification; reconcile it first')
            write_journal(directory, {'previous': previous, 'published': published, 'qualified': True})
            return None
        if state.get('status') in ('prepared', 'advertised'):
            raise ValueError('An unfinished store promotion must be recovered first')
        data: Object = {'previous': previous, 'published': published, 'status': 'prepared'}
        prepared = self.commit(checkout, data, before)
        advertised = self.commit(checkout, {**data, 'status': 'advertised'}, prepared)
        write_journal(
            directory,
            {
                'previous': previous,
                'published': published,
                'qualified': False,
                'fence_before': before,
                'fence_prepared': prepared,
                'fence_advertised': advertised,
            },
        )
        self.push(checkout, before, prepared)
        return prepared, advertised

    def recover(self, checkout: Path, data: Object, allowed: set[str]) -> None:
        previous, published = oid(data['previous'], empty=True), oid(data['published'])
        for _ in range(3):
            fence = self.head(self.fence)
            state = self.state(checkout, fence) if fence else {}
            same = state.get('previous') == previous and state.get('published') == published
            if same and state.get('status') in ('aborted', 'qualified'):
                return
            parent = self.git('show', '-s', '--format=%P', fence, cwd=checkout) if fence else ''
            if fence not in allowed and not (same and state.get('status') == 'advertised' and parent in allowed):
                raise ValueError('Store promotion state changed; refusing unrelated recovery')
            current = self.head(self.branch)
            if current not in (previous, published):
                raise ValueError('Store advertisement changed; refusing to roll back unrelated metadata')
            self.fetch(checkout, current)
            aborted = self.commit(checkout, {'previous': previous, 'published': published, 'status': 'aborted'}, fence)
            try:
                self.push(checkout, fence, aborted, advertisement=(current, previous) if current != previous else None)
                return
            except subprocess.CalledProcessError:
                if self.head(self.fence) == fence:
                    raise
        raise ValueError('Store promotion kept changing during recovery')

    def reconcile(self, directory: Path) -> None:
        with tempfile.TemporaryDirectory(prefix='.hass-recovery.', dir=directory) as temporary:
            checkout = Path(temporary)
            self.git('init', '-q', str(checkout))
            fence = self.head(self.fence)
            state = self.state(checkout, fence) if fence else {}
            if state.get('status') in ('prepared', 'advertised'):
                self.recover(checkout, state, {fence})

    def rollback(self, directory: Path) -> None:
        journal = directory / 'promotion.json'
        if not journal.is_file():
            return
        data = object_value(read_json(journal))
        if data.get('qualified') is True:
            return
        allowed = {oid(data['fence_before'], empty=True), oid(data['fence_prepared']), oid(data['fence_advertised'])}
        with tempfile.TemporaryDirectory(prefix='.hass-rollback.', dir=directory.parent) as temporary:
            checkout = Path(temporary)
            self.git('init', '-q', str(checkout))
            self.recover(checkout, data, allowed)

    def qualify(self, directory: Path) -> None:
        data = object_value(read_json(directory / 'promotion.json'))
        if data.get('qualified') is True:
            return
        with tempfile.TemporaryDirectory(prefix='.hass-qualified.', dir=directory) as temporary:
            checkout = Path(temporary)
            self.git('init', '-q', str(checkout))
            advertised = text(data['fence_advertised'])
            state = self.state(checkout, advertised)
            if self.head(self.branch) != data['published']:
                raise ValueError('Store advertisement changed during qualification')
            qualified = self.commit(checkout, {**state, 'status': 'qualified'}, advertised)
            self.push(checkout, advertised, qualified)
        write_journal(directory, {**data, 'qualified': True}, replace=True)
