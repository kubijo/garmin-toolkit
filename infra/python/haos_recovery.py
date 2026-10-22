"""Recover failed Supervisor acceptance through production leases in the private store."""

import json
import re
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

import yaml

from haos_repository import local_store
from hass_publish import push_metadata
from hass_store import oid, write_journal
from json_data import Object, file_sha256, object_value, read_json, regular_file, text, unique_object


def release_assets(directory: Path, archive: Path) -> tuple[tuple[Path, str], ...]:
    """Remember the authenticated inputs whose bytes must survive the recovery check."""
    paths = (
        archive,
        *(
            directory / name
            for name in ('release.json', 'SHA256SUMS', 'checksums.sigstore.jsonl', 'artifacts.sigstore.jsonl')
        ),
    )
    for path in paths:
        regular_file(path)
    return tuple((path, file_sha256(path)) for path in paths)


def update_failure(result: subprocess.CompletedProcess[str], app_slug: str) -> Object:
    """Validate Supervisor's structured update error; retain its human message as diagnostics."""
    response = object_value(json.loads(result.stdout, object_pairs_hook=unique_object))
    # The CLI's raw JSON mode exits successfully after delivering an API error body.
    if (
        result.returncode != 0
        or response.get('result') != 'error'
        or response.get('error_key') != 'app_unknown_error'
        or object_value(response.get('extra_fields')).get('app') != app_slug
    ):
        raise RuntimeError('Supervisor did not report the expected update error for the recovery app')
    text(response.get('message'))
    return {'returncode': result.returncode, 'response': response}


@dataclass(frozen=True)
class Recovery:
    runtime: Path
    evidence: Path

    @property
    def state_path(self) -> Path:
        return self.runtime / 'recovery-state.json'

    def prepare(self, version: str) -> None:
        owner = local_store(self.runtime)
        if owner.head(owner.fence) or self.state_path.exists():
            raise ValueError('Disposable recovery state already exists')
        checkout = self.runtime / 'repository-work'
        metadata = self.runtime / 'recovery-metadata'
        shutil.copytree(checkout, metadata, ignore=shutil.ignore_patterns('.git'))
        config_path = metadata / 'garmin_toolkit/config.yaml'
        config = object_value(yaml.safe_load(config_path.read_text()))
        previous = text(config['version'])
        if any(re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', value) is None for value in (previous, version)) or tuple(
            map(int, version.split('.'))
        ) <= tuple(map(int, previous.split('.'))):
            raise ValueError('Recovery requires a greater numeric published candidate version')
        baseline = oid(owner.head(owner.branch))
        journal = self.evidence / 'recovery-baseline'
        journal.mkdir()
        fence = owner.prepare(checkout, journal, '', baseline)
        if fence is None:
            raise RuntimeError('Disposable baseline did not prepare publication state')
        owner.push(checkout, *fence, advertisement=(baseline, baseline))
        owner.qualify(journal)
        config['version'] = version
        config_path.write_text(yaml.safe_dump(config, sort_keys=False))
        write_journal(self.runtime, {'baseline': baseline, 'version': version}, name=self.state_path.name)

    def advertise(self, attempt: str) -> None:
        if attempt not in ('failed', 'retry', 'qualified-retry'):
            raise ValueError('Unknown disposable recovery attempt')
        owner = local_store(self.runtime)
        state = object_value(read_json(self.state_path))
        if attempt == 'retry':
            restored = object_value(read_json(self.runtime / 'recovery-rollback.json'))
            if owner.head(owner.branch) != restored['baseline'] or owner.head(owner.fence) != restored['state']:
                raise ValueError('Recovery retry requires the unchanged rolled-back private publication')
        journal = self.evidence / ('recovery-' + attempt)
        journal.mkdir()
        push_metadata(
            self.runtime / 'recovery-metadata',
            owner.branch,
            oid(state['baseline']),
            resume=True,
            before_push=lambda commit, current, checkout: owner.prepare(checkout, journal, current, commit),
            owner=owner,
        )

    def rollback(self) -> None:
        owner = local_store(self.runtime)
        owner.rollback(self.evidence / 'recovery-failed')
        if owner.head(owner.branch) != object_value(read_json(self.state_path))['baseline']:
            raise RuntimeError('Failed installation recovery did not restore the exact baseline metadata')
        checkout = self.runtime / 'repository-work'
        if owner.state(checkout, owner.head(owner.fence))['status'] != 'aborted':
            raise RuntimeError('Failed installation recovery did not advance the durable recovery fence')
        write_journal(
            self.runtime,
            {'baseline': owner.head(owner.branch), 'state': owner.head(owner.fence)},
            replace=True,
            name='recovery-rollback.json',
        )

    def qualify(self) -> Object:
        owner = local_store(self.runtime)
        if (
            object_value(read_json(self.runtime / 'recovery-rollback.json'))['baseline']
            != object_value(read_json(self.state_path))['baseline']
        ):
            raise ValueError('Recovery baseline differs from its rollback evidence')
        owner.qualify(self.evidence / 'recovery-retry')
        qualified = owner.head(owner.branch), owner.head(owner.fence)
        try:
            owner.rollback(self.evidence / 'recovery-failed')
        except ValueError as error:
            if 'unrelated recovery' not in str(error):
                raise
        self.advertise('qualified-retry')
        owner.qualify(self.evidence / 'recovery-qualified-retry')
        if (owner.head(owner.branch), owner.head(owner.fence)) != qualified:
            raise RuntimeError('A stale rollback or qualified retry changed the private publication')
        return {
            'baseline_metadata_restored': True,
            'retry_qualified': True,
            'qualified_retry_preserved': True,
            'stale_rollback_preserved': True,
            'scope': 'disposable local Git publication',
        }
