"""Exercise production store recovery with synthetic metadata on caller-owned refs."""

import subprocess
from dataclasses import dataclass, replace
from pathlib import Path

import yaml

from hass_publish import WithdrawPublication, push_metadata, withdraw_publication
from hass_store import STATE_FILE, Store
from hass_store_migration import CheckpointPublication, checkpoint_store
from json_data import Object, json_bytes, object_value, read_json, text


class InterruptedClient(RuntimeError):
    """An injected client failure around a real Git push."""


@dataclass(frozen=True)
class StoreDrill:
    owner: Store
    root: Path

    def heads(self) -> tuple[str, str]:
        return self.owner.head(self.owner.branch), self.owner.head(self.owner.fence)

    def state(self) -> Object:
        return self.owner.state(self.root / 'checkout', self.owner.head(self.owner.fence))

    def metadata(self, version: str) -> Path:
        path = self.root / 'metadata'
        app = path / 'garmin_toolkit'
        app.mkdir(parents=True, exist_ok=True)
        (path / 'repository.yaml').write_text('name: Synthetic recovery fixture\n')
        (app / 'config.yaml').write_text(
            yaml.safe_dump({'slug': 'garmin_toolkit', 'version': version, 'description': 'Synthetic recovery fixture'})
        )
        return path

    def advertise(self, name: str, previous: str, *, owner: Store | None = None) -> str:
        owner = owner or self.owner
        journal = self.root / name
        journal.mkdir()
        return push_metadata(
            self.root / 'metadata',
            owner.branch,
            previous,
            resume=True,
            before_push=lambda commit, current, checkout: owner.prepare(checkout, journal, current, commit),
            owner=owner,
        )

    def assert_heads(self, expected: tuple[str, str]) -> None:
        if self.heads() != expected:
            raise RuntimeError('Recovery changed unrelated or already qualified fixture refs')

    def checkpoint(self) -> Object:
        checkout = self.root / 'checkout'
        before, fence = self.heads()
        self.owner.fetch(checkout, fence)
        empty = self.owner.git('hash-object', '-t', 'tree', '-w', '/dev/null', cwd=checkout)
        legacy = self.owner.commit_tree(checkout, empty, fence, json_bytes(self.state()).decode())
        self.owner.push(checkout, fence, legacy)
        options = CheckpointPublication(store_commit=before, state_commit=legacy, output=self.root / 'checkpoint')

        def lost_reply(*arguments: str, cwd: Path | None = None) -> str:
            result = self.owner.git(*arguments, cwd=cwd)
            if 'push' in arguments:
                raise InterruptedClient('Injected lost reply after the server accepted the checkpoint')
            return result

        try:
            checkpoint_store(options, owner=replace(self.owner, git=lost_reply))
        except InterruptedClient:
            pass
        else:
            raise RuntimeError('The checkpoint reply was not discarded')
        after = self.heads()
        checkpoint_store(options, owner=self.owner)
        self.assert_heads(after)
        self.owner.fetch(checkout, after[0])
        state = self.state()
        if (
            before == after[0]
            or legacy == after[1]
            or state.get('status') != 'qualified'
            or state.get('published') != after[0]
            or state.get('checkpoint_of') != legacy
            or self.owner.git('show', '-s', '--format=%P', after[1], cwd=checkout)
            or self.owner.git('ls-tree', '--name-only', after[1], cwd=checkout) != STATE_FILE
            or self.owner.git('rev-parse', before + '^{tree}', cwd=checkout)
            != self.owner.git('rev-parse', after[0] + '^{tree}', cwd=checkout)
        ):
            raise RuntimeError('Checkpoint failed to retain qualified metadata with a fresh state-file history')
        return {'history_replaced': True, 'metadata_preserved': True, 'lost_reply_retried': True}

    def withdrawal(self) -> Object:
        before, fence = self.heads()
        options = WithdrawPublication(
            version='0.0.3',
            store_commit=before,
            state_commit=fence,
            reason='Synthetic post-checkpoint withdrawal',
            output=self.root / 'withdrawal',
        )
        withdraw_publication(options, owner=self.owner)
        withdrawn = self.heads()
        withdraw_publication(replace(options, output=self.root / 'withdrawal-retry'), owner=self.owner)
        self.assert_heads(withdrawn)
        checkout = self.root / 'checkout'
        state = self.state()
        self.owner.fetch(checkout, withdrawn[0])
        if (
            state.get('status') != 'withdrawn'
            or self.owner.git('ls-tree', '--name-only', withdrawn[0], cwd=checkout) != 'repository.yaml'
            or self.owner.git('rev-parse', before + ':repository.yaml', cwd=checkout)
            != self.owner.git('rev-parse', withdrawn[0] + ':repository.yaml', cwd=checkout)
        ):
            raise RuntimeError('Withdrawal failed to remove only the app and preserve repository metadata')
        try:
            self.advertise('withdrawal-stale', before)
        except ValueError as error:
            if 'unrelated metadata' not in str(error):
                raise
        else:
            raise RuntimeError('A stale promotion restored withdrawn fixture metadata')
        self.assert_heads(withdrawn)
        self.metadata('0.0.4')
        self.advertise('withdrawal-replacement', withdrawn[0])
        self.owner.qualify(self.root / 'withdrawal-replacement')
        replacement = self.heads()
        self.advertise('withdrawal-replacement-retry', withdrawn[0])
        self.owner.qualify(self.root / 'withdrawal-replacement-retry')
        self.assert_heads(replacement)
        if self.state().get('status') != 'qualified':
            raise RuntimeError('Post-withdrawal replacement failed qualification')
        return {
            'app_removed': True,
            'retry_preserved': True,
            'stale_promotion_rejected': True,
            'replacement_qualified': True,
        }

    def run(self) -> Object:
        self.root.mkdir()
        checkout = self.root / 'checkout'
        self.owner.git('init', '-q', str(checkout))
        if self.heads() != ('', ''):
            raise ValueError('Store recovery requires fresh fixture refs')
        self.metadata('0.0.0')
        self.advertise('baseline', '')
        self.owner.qualify(self.root / 'baseline')
        checkpoint = self.checkpoint()
        baseline = self.heads()[0]

        # Retain both push sources after the temporary publication checkout disappears.
        self.metadata('0.0.1')
        prepared = self.root / 'prepared'
        prepared.mkdir()

        def interrupt(commit: str, current: str, working: Path) -> tuple[str, str] | None:
            self.owner.prepare(working, prepared, current, commit)
            journal = object_value(read_json(prepared / 'promotion.json'))
            self.owner.git('fetch', str(working), commit, text(journal['fence_advertised']), cwd=checkout)
            raise InterruptedClient('Injected interruption after prepare, before advertisement')

        try:
            push_metadata(self.root / 'metadata', self.owner.branch, baseline, before_push=interrupt, owner=self.owner)
        except InterruptedClient:
            pass
        else:
            raise RuntimeError('The prepared fixture did not interrupt')
        if self.state()['status'] != 'prepared' or self.heads()[0] != baseline:
            raise RuntimeError('Interrupted preparation changed the advertisement')
        self.owner.reconcile(self.root)
        recovered = self.heads()
        journal = object_value(read_json(prepared / 'promotion.json'))
        for name in ('published', 'fence_advertised'):
            self.owner.git('cat-file', '-e', text(journal[name]) + '^{commit}', cwd=checkout)
        try:
            self.owner.push(
                checkout,
                text(journal['fence_prepared']),
                text(journal['fence_advertised']),
                advertisement=(baseline, text(journal['published'])),
            )
        except subprocess.CalledProcessError as error:
            if not any(
                '[rejected]' in line and line.endswith(' -> ' + self.owner.fence + ' (stale info)')
                for line in (error.stderr or '').splitlines()
            ):
                raise
        else:
            raise RuntimeError('A delayed advertisement bypassed the recovery fence')
        self.assert_heads(recovered)
        if recovered[0] != baseline or self.state()['status'] != 'aborted':
            raise RuntimeError('Prepared recovery did not retain the baseline')

        dropped = False

        def lost_reply(*arguments: str, cwd: Path | None = None) -> str:
            nonlocal dropped
            result = self.owner.git(*arguments, cwd=cwd)
            if (
                not dropped
                and 'push' in arguments
                and any(arg.endswith(':refs/heads/' + self.owner.branch) for arg in arguments)
            ):
                dropped = True
                raise InterruptedClient('Injected lost reply after the server accepted the atomic advertisement')
            return result

        try:
            self.advertise('lost-reply', baseline, owner=replace(self.owner, git=lost_reply))
        except InterruptedClient:
            pass
        else:
            raise RuntimeError('The atomic advertisement reply was not discarded')
        if not dropped or self.heads()[0] == baseline or self.state()['status'] != 'advertised':
            raise RuntimeError('Lost-reply injection did not follow a real atomic advertisement')
        self.owner.reconcile(self.root)
        if self.heads()[0] != baseline or self.state()['status'] != 'aborted':
            raise RuntimeError('Lost-reply recovery did not restore the baseline')

        self.advertise('failed-qualification', baseline)
        # Model the failure result at the Supervisor acceptance boundary; no installation is claimed here.
        self.owner.rollback(self.root / 'failed-qualification')
        if self.heads()[0] != baseline or self.state()['status'] != 'aborted':
            raise RuntimeError('Failed qualification did not restore the baseline')
        candidate = self.advertise('retry', baseline)
        self.owner.qualify(self.root / 'retry')
        qualified = self.heads()
        if qualified[0] != candidate or self.state()['status'] != 'qualified':
            raise RuntimeError('The replacement fixture did not qualify')
        self.advertise('qualified-retry', baseline)
        self.owner.qualify(self.root / 'qualified-retry')
        try:
            self.owner.rollback(self.root / 'failed-qualification')
        except ValueError as error:
            if 'unrelated recovery' not in str(error):
                raise
        self.assert_heads(qualified)

        self.metadata('0.0.2')
        attempted = self.advertise('concurrent', candidate)
        self.metadata('0.0.3')
        competing = push_metadata(self.root / 'metadata', self.owner.branch, attempted, owner=self.owner)
        before = self.heads()
        try:
            self.owner.rollback(self.root / 'concurrent')
        except ValueError as error:
            if 'unrelated metadata' not in str(error):
                raise
        else:
            raise RuntimeError('Recovery replaced competing fixture metadata')
        self.assert_heads(before)
        self.owner.fetch(checkout, before[1])
        competing_state = self.owner.commit(
            checkout, {'previous': competing, 'published': competing, 'status': 'qualified'}, before[1]
        )
        self.owner.push(checkout, before[1], competing_state)
        before = self.heads()
        try:
            self.owner.rollback(self.root / 'concurrent')
        except ValueError as error:
            if 'unrelated recovery' not in str(error):
                raise
        else:
            raise RuntimeError('Recovery replaced competing fixture state')
        self.assert_heads(before)
        withdrawal = self.withdrawal()
        before = self.heads()
        result: Object = {
            'checkpoint': checkpoint,
            'withdrawal': withdrawal,
            'prepared_interruption': True,
            'delayed_advertisement_rejected': True,
            'lost_atomic_reply': True,
            'failed_qualification_rollback': True,
            'retry_qualified': True,
            'qualified_retry_preserved': True,
            'concurrent_metadata_preserved': True,
            'concurrent_state_preserved': True,
            'installation_failure': 'injected acceptance result; no Supervisor installation',
            'heads': {'advertisement': before[0], 'state': before[1]},
        }
        (self.root / 'result.json').write_bytes(json_bytes(result))
        return result
