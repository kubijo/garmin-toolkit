"""Exercise production withdrawal in the disposable guest's Git repository."""

import re
import shutil
from dataclasses import dataclass, replace
from pathlib import Path

import yaml

from haos_repository import git
from hass_publish import WithdrawPublication, push_metadata, withdraw_publication
from hass_store import FENCE, Store, oid
from json_data import Object, json_bytes, object_value, read_json, text


@dataclass(frozen=True)
class Withdrawal:
    runtime: Path
    evidence: Path

    def store(self) -> Store:
        remote = self.runtime / 'repository-http/garmin-toolkit.git'
        if remote.is_symlink() or not remote.is_dir() or not remote.resolve().is_relative_to(self.runtime.resolve()):
            raise ValueError('Withdrawal acceptance requires its owned local Git repository')

        def execute(*arguments: str, cwd: Path | None = None) -> str:
            return git(self.runtime, *arguments, cwd=cwd)

        def head(branch: str) -> str:
            result = execute('ls-remote', '--refs', str(remote), 'refs/heads/' + branch)
            if not result:
                return ''
            commit, _, reference = result.partition('\t')
            if reference != 'refs/heads/' + branch:
                raise ValueError('Unexpected disposable publication ref')
            return oid(commit)

        return Store(str(remote), execute, head)

    def withdraw(self, version: str) -> None:
        owner = self.store()
        checkout = self.runtime / 'repository-work'
        if owner.head(FENCE):
            raise ValueError('Disposable withdrawal state already exists')
        metadata = self.runtime / 'withdrawal-metadata'
        shutil.copytree(checkout, metadata, ignore=shutil.ignore_patterns('.git'))
        initial = owner.head('hass-apps')
        journal = self.evidence / 'withdrawal-baseline'
        journal.mkdir()
        fence = owner.prepare(checkout, journal, '', initial)
        if fence is None:
            raise RuntimeError('Disposable baseline did not prepare publication state')
        owner.push(checkout, *fence, advertisement=(initial, initial))
        owner.qualify(journal)
        options = WithdrawPublication(
            version=version,
            store_commit=initial,
            state_commit=owner.head(FENCE),
            reason='Disposable Supervisor withdrawal acceptance',
            output=self.evidence / 'withdrawal',
        )
        withdraw_publication(options, owner=owner)
        withdrawn, state = owner.head('hass-apps'), owner.head(FENCE)
        withdraw_publication(replace(options, output=self.evidence / 'withdrawal-retry'), owner=owner)
        if (owner.head('hass-apps'), owner.head(FENCE)) != (withdrawn, state):
            raise RuntimeError('A completed withdrawal retry changed disposable publication refs')
        document: Object = {
            'version': version,
            'previous': initial,
            'withdrawn': withdrawn,
            'state': state,
            'original_previous': '',
            'idempotent_retry': True,
        }
        (self.runtime / 'withdrawal-state.json').write_bytes(json_bytes(document))
        self.reject_stale_retry()

    def reject_stale_retry(self) -> None:
        owner = self.store()
        before = owner.head('hass-apps'), owner.head(FENCE)
        state = object_value(read_json(self.runtime / 'withdrawal-state.json'))
        try:
            push_metadata(
                self.runtime / 'withdrawal-metadata',
                'hass-apps',
                oid(state['original_previous'], empty=True),
                resume=True,
                owner=owner,
            )
        except ValueError as error:
            if 'unrelated metadata' not in str(error):
                raise
        else:
            raise RuntimeError('A stale promotion restored the withdrawn app metadata')
        if (owner.head('hass-apps'), owner.head(FENCE)) != before:
            raise RuntimeError('Stale promotion changed disposable publication refs')

    def replace(self, version: str) -> None:
        state = object_value(read_json(self.runtime / 'withdrawal-state.json'))
        if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version) or tuple(map(int, version.split('.'))) <= tuple(
            map(int, text(state['version']).split('.'))
        ):
            raise ValueError('Withdrawal replacement requires a greater numeric private version')
        owner = self.store()
        metadata = self.runtime / 'replacement-metadata'
        shutil.copytree(self.runtime / 'withdrawal-metadata', metadata)
        config_path = metadata / 'garmin_toolkit/config.yaml'
        config = object_value(yaml.safe_load(config_path.read_text()))
        config['version'] = version
        config_path.write_text(yaml.safe_dump(config, sort_keys=False))
        journal = self.evidence / 'withdrawal-replacement'
        journal.mkdir()
        push_metadata(
            metadata,
            'hass-apps',
            text(state['withdrawn']),
            resume=True,
            before_push=lambda commit, current, checkout: owner.prepare(checkout, journal, current, commit),
            owner=owner,
        )

    def qualify(self) -> Object:
        self.store().qualify(self.evidence / 'withdrawal-replacement')
        self.reject_stale_retry()
        return {
            'store_refreshed': True,
            'installed_app_preserved': True,
            'populated_data_preserved': True,
            'downloads_preserved': True,
            'replacement_installed': True,
            'replacement_qualified': True,
            'idempotent_retry': True,
            'stale_retry_rejected': True,
            'scope': 'disposable Supervisor and local Git; public GitHub/GHCR refs untouched',
        }
