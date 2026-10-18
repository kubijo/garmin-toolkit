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


def oid(value: object, *, empty: bool = False) -> str:
    if not isinstance(value, str) or re.fullmatch(r'(?:[0-9a-f]{40})?' if empty else r'[0-9a-f]{40}', value) is None:
        raise ValueError('Invalid store promotion object ID')
    return value


class Git(Protocol):
    def __call__(self, *arguments: str, cwd: Path | None = None) -> str: ...


def write_journal(directory: Path, data: Object, *, replace: bool = False) -> None:
    with tempfile.TemporaryDirectory(prefix='.promotion.', dir=directory) as temporary:
        staged, journal = Path(temporary) / 'promotion.json', directory / 'promotion.json'
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

    def fetch(self, checkout: Path, commit: str) -> None:
        if commit:
            self.git('fetch', '--no-tags', '--depth=2', self.remote, commit, cwd=checkout)

    def state(self, checkout: Path, commit: str) -> Object:
        self.fetch(checkout, commit)
        data = object_value(json.loads(self.git('show', '-s', '--format=%B', commit, cwd=checkout)))
        if data.get('status') not in ('prepared', 'advertised', 'qualified', 'aborted'):
            raise ValueError('Invalid durable store promotion state')
        oid(data['previous'], empty=True)
        oid(data['published'])
        return data

    def commit(self, checkout: Path, data: Object, parent: str) -> str:
        tree = self.git('hash-object', '-t', 'tree', '-w', '/dev/null', cwd=checkout)
        return self.git(
            '-c',
            'user.name=Garmin release',
            '-c',
            'user.email=release@users.noreply.github.com',
            'commit-tree',
            tree,
            *(['-p', parent] if parent else []),
            '-m',
            json_bytes(data).decode(),
            cwd=checkout,
        )

    def push(self, checkout: Path, before: str, after: str, *, advertisement: tuple[str, str] | None = None) -> None:
        self.git(
            'push',
            '--atomic',
            '--force-with-lease=refs/heads/' + FENCE + ':' + before,
            *(['--force-with-lease=refs/heads/hass-apps:' + advertisement[0]] if advertisement else []),
            self.remote,
            after + ':refs/heads/' + FENCE,
            *([advertisement[1] + ':refs/heads/hass-apps'] if advertisement else []),
            cwd=checkout,
        )

    def prepare(self, checkout: Path, directory: Path, previous: str, published: str) -> tuple[str, str] | None:
        oid(previous, empty=True)
        oid(published)
        before = self.head(FENCE)
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
            fence = self.head(FENCE)
            state = self.state(checkout, fence) if fence else {}
            same = state.get('previous') == previous and state.get('published') == published
            if same and state.get('status') in ('aborted', 'qualified'):
                return
            parent = self.git('show', '-s', '--format=%P', fence, cwd=checkout) if fence else ''
            if fence not in allowed and not (same and state.get('status') == 'advertised' and parent in allowed):
                raise ValueError('Store promotion state changed; refusing unrelated recovery')
            current = self.head('hass-apps')
            if current not in (previous, published):
                raise ValueError('Store advertisement changed; refusing to roll back unrelated metadata')
            self.fetch(checkout, current)
            aborted = self.commit(checkout, {'previous': previous, 'published': published, 'status': 'aborted'}, fence)
            try:
                self.push(checkout, fence, aborted, advertisement=(current, previous) if current != previous else None)
                return
            except subprocess.CalledProcessError:
                if self.head(FENCE) == fence:
                    raise
        raise ValueError('Store promotion kept changing during recovery')

    def reconcile(self, directory: Path) -> None:
        with tempfile.TemporaryDirectory(prefix='.hass-recovery.', dir=directory) as temporary:
            checkout = Path(temporary)
            self.git('init', '-q', str(checkout))
            fence = self.head(FENCE)
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
            if self.head('hass-apps') != data['published']:
                raise ValueError('Store advertisement changed during qualification')
            qualified = self.commit(checkout, {**state, 'status': 'qualified'}, advertised)
            self.push(checkout, advertised, qualified)
        write_journal(directory, {**data, 'qualified': True}, replace=True)
