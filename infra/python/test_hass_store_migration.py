"""Real Git state-file compatibility and exact-leased publication history cutover."""

import json
import subprocess
import tempfile
import unittest
from contextlib import chdir
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import hass_store_migration as migration
from haos_repository import git as isolated_git
from hass_store import STATE_FILE, Store
from json_data import Object, json_bytes, object_value, read_json


class StoreMigrationTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.remote, self.checkout = self.root / 'remote.git', self.root / 'checkout'
        self.checkout.mkdir()
        self.git('init', '--bare', '-q', str(self.remote))
        self.git('init', '-q', str(self.checkout))
        (self.checkout / 'README.md').write_text('Synthetic Store metadata\n')
        self.git('add', '.', cwd=self.checkout)
        self.git('commit', '-qm', 'Synthetic Store metadata', cwd=self.checkout)
        self.published = self.git('rev-parse', 'HEAD', cwd=self.checkout)

        def head(branch: str) -> str:
            result = self.git('ls-remote', '--refs', str(self.remote), 'refs/heads/' + branch)
            return result.partition('\t')[0] if result else ''

        self.owner = Store(str(self.remote), self.git, head)
        self.state: Object = {'previous': '', 'published': self.published, 'status': 'qualified'}
        self.legacy = self.legacy_commit(self.state)
        self.git(
            'push',
            str(self.remote),
            self.published + ':refs/heads/hass-apps',
            self.legacy + ':refs/heads/hass-publication',
            self.published + ':refs/heads/main',
            self.published + ':refs/tags/v0.0.0',
            cwd=self.checkout,
        )
        self.options = migration.CheckpointPublication(
            store_commit=self.published, state_commit=self.legacy, output=self.root / 'evidence'
        )
        self.idle = patch('hass_store_migration.command', return_value='[{"workflow_runs":[{"status":"completed"}]}]')
        self.idle.start()
        self.addCleanup(self.idle.stop)

    def git(self, *arguments: str, cwd: Path | None = None) -> str:
        return isolated_git(self.root, *arguments, cwd=cwd or self.checkout)

    def legacy_commit(self, data: Object, *, tree: str | None = None) -> str:
        tree = tree or self.git('hash-object', '-t', 'tree', '-w', '/dev/null')
        return self.git('commit-tree', tree, '-m', json_bytes(data).decode())

    def heads(self) -> tuple[str, str]:
        return self.owner.head(self.owner.branch), self.owner.head(self.owner.fence)

    def test_readable_state_file_retains_legacy_reads_and_never_changes_the_metadata_index(self) -> None:
        before = self.git('write-tree')
        self.assertEqual(self.owner.state(self.checkout, self.legacy), self.state)
        modern = self.owner.commit(self.checkout, self.state, self.legacy)
        self.assertEqual(self.owner.state(self.checkout, modern, fetch=False), self.state)
        self.assertEqual(self.git('show', modern + ':' + STATE_FILE), json_bytes(self.state).decode().strip())
        self.assertEqual(self.git('show', '-s', '--format=%s', modern), 'Qualify HASS release')
        self.assertEqual(self.git('ls-tree', '--name-only', modern), STATE_FILE)
        self.assertEqual(self.git('write-tree'), before)
        self.assertEqual(self.git('status', '--porcelain'), '')

    def test_nonempty_unrecognized_trees_cannot_fall_back_to_a_json_commit_message(self) -> None:
        tree = self.git('rev-parse', self.published + '^{tree}')
        malformed = self.legacy_commit(self.state, tree=tree)
        with self.assertRaisesRegex(ValueError, 'publication.json'):
            self.owner.state(self.checkout, malformed, fetch=False)

    def test_checkpoint_discards_json_history_preserves_store_files_and_retries_without_new_commits(self) -> None:
        protected = self.git('ls-remote', '--refs', str(self.remote), 'refs/heads/main', 'refs/tags/v*')
        migration.checkpoint_publication(self.options, owner=self.owner)
        after_store, after_state = self.heads()
        self.assertNotEqual(after_store, self.published)
        self.assertNotEqual(after_state, self.legacy)
        self.assertEqual(
            self.git('ls-remote', '--refs', str(self.remote), 'refs/heads/main', 'refs/tags/v*'), protected
        )
        scratch = self.root / 'verification'
        self.git('init', '-q', str(scratch))
        self.git('fetch', '--no-tags', str(self.remote), after_store, after_state, cwd=scratch)
        self.assertEqual(self.git('show', '-s', '--format=%P', after_state, cwd=scratch), '')
        self.assertEqual(self.git('rev-list', after_state, cwd=scratch), after_state)
        self.assertEqual(self.git('show', '-s', '--format=%P', after_store, cwd=scratch), self.published)
        self.assertEqual(
            self.git('rev-parse', after_store + '^{tree}', cwd=scratch),
            self.git('rev-parse', self.published + '^{tree}'),
        )
        self.assertEqual(
            self.owner.state(scratch, after_state),
            {**self.state, 'published': after_store, 'checkpoint_of': self.legacy},
        )
        self.assertEqual(
            self.git('show', '-s', '--format=%s', after_state, cwd=scratch), 'Checkpoint HASS publication state'
        )
        migration.checkpoint_publication(self.options, owner=self.owner)
        self.assertEqual(self.heads(), (after_store, after_state))
        self.assertIs(object_value(read_json(self.options.output / 'checkpoint.json'))['completed'], True)

    def test_lost_replies_retry_the_exact_checkpoint_before_or_after_server_acceptance(self) -> None:
        for mode in ('before', 'after'):
            with self.subTest(mode=mode):
                options = replace(self.options, output=self.root / mode)
                self.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/hass-apps', self.published)
                self.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/hass-publication', self.legacy)

                def fail(*arguments: str, cwd: Path | None = None, mode: str = mode) -> str:
                    if 'push' not in arguments:
                        return self.git(*arguments, cwd=cwd)
                    if mode == 'after':
                        self.git(*arguments, cwd=cwd)
                    raise subprocess.TimeoutExpired(arguments, 30)

                with self.assertRaises(subprocess.TimeoutExpired):
                    migration.checkpoint_publication(options, owner=replace(self.owner, git=fail))
                report = object_value(read_json(options.output / 'checkpoint.json'))
                self.assertIs(report['completed'], False)
                migration.checkpoint_publication(options, owner=self.owner)
                self.assertEqual(self.heads(), (report['after_store'], report['after_state']))
                self.assertIs(object_value(read_json(options.output / 'checkpoint.json'))['completed'], True)

    def test_atomic_leases_preserve_competing_store_or_state_changes(self) -> None:
        tree = self.git('rev-parse', self.published + '^{tree}')
        competing = self.owner.commit_tree(self.checkout, tree, self.published, 'Concurrent fixture metadata')
        self.git('push', str(self.remote), competing + ':refs/heads/competing-fixture')
        for branch in (self.owner.branch, self.owner.fence):
            with self.subTest(branch=branch):
                self.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/hass-apps', self.published)
                self.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/hass-publication', self.legacy)
                options = replace(self.options, output=self.root / ('concurrent-' + branch))

                def race(*arguments: str, cwd: Path | None = None, branch: str = branch) -> str:
                    if 'push' in arguments:
                        self.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/' + branch, competing)
                    return self.git(*arguments, cwd=cwd)

                with self.assertRaises(subprocess.CalledProcessError):
                    migration.checkpoint_publication(options, owner=replace(self.owner, git=race))
                expected = (competing, self.legacy) if branch == self.owner.branch else (self.published, competing)
                self.assertEqual(self.heads(), expected)
                with self.assertRaisesRegex(ValueError, 'unrelated checkpoint'):
                    migration.checkpoint_publication(options, owner=self.owner)
                self.assertEqual(self.heads(), expected)

    def test_relative_paths_work_for_state_writes_and_checkpoint_retries(self) -> None:
        with chdir(self.root):
            modern = self.owner.commit(Path('checkout'), self.state, self.legacy)
            self.assertEqual(self.owner.state(self.checkout, modern, fetch=False), self.state)
            options = replace(self.options, output=Path('relative-evidence'))
            migration.checkpoint_publication(options, owner=self.owner)
            after = self.heads()
            migration.checkpoint_publication(options, owner=self.owner)
            self.assertEqual(self.heads(), after)
            self.assertIs(object_value(read_json(options.output / 'checkpoint.json'))['completed'], True)

    def test_checkpoint_retains_normal_promotion_and_qualification(self) -> None:
        migration.checkpoint_publication(self.options, owner=self.owner)
        previous, before_state = self.heads()
        self.owner.fetch(self.checkout, previous)
        tree = self.git('rev-parse', previous + '^{tree}')
        published = self.owner.commit_tree(self.checkout, tree, previous, 'Following synthetic release')
        directory = self.root / 'following'
        directory.mkdir()
        prepared = self.owner.prepare(self.checkout, directory, previous, published)
        self.assertIsNotNone(prepared)
        assert prepared is not None
        self.assertEqual(self.git('show', '-s', '--format=%P', prepared[0]), before_state)
        self.owner.push(self.checkout, prepared[0], prepared[1], advertisement=(previous, published))
        self.owner.qualify(directory)
        self.assertEqual(
            self.owner.state(self.checkout, self.heads()[1]),
            {'previous': previous, 'published': published, 'status': 'qualified'},
        )
        self.assertEqual(self.heads()[0], published)

    def test_modified_retry_journals_cannot_publish_checkpoint_objects(self) -> None:
        def fail(*arguments: str, cwd: Path | None = None) -> str:
            if 'push' in arguments:
                raise subprocess.TimeoutExpired(arguments, 30)
            return self.git(*arguments, cwd=cwd)

        for field in ('remote', 'metadata_tree', 'after_state'):
            with self.subTest(field=field):
                options = replace(self.options, output=self.root / ('modified-' + field))
                with self.assertRaises(subprocess.TimeoutExpired):
                    migration.checkpoint_publication(options, owner=replace(self.owner, git=fail))
                journal = options.output / 'checkpoint.json'
                report = object_value(read_json(journal))
                journal.write_bytes(json_bytes({**report, field: self.legacy}))
                with self.assertRaises(ValueError):
                    migration.checkpoint_publication(options, owner=self.owner)
                self.assertEqual(self.heads(), (self.published, self.legacy))

    def test_active_or_queued_workflows_block_before_any_checkpoint_mutation(self) -> None:
        for status in ('in_progress', 'queued', 'waiting', 'pending', 'requested'):
            with (
                self.subTest(status=status),
                patch(
                    'hass_store_migration.command', return_value=json.dumps([{'workflow_runs': [{'status': status}]}])
                ),
                self.assertRaisesRegex(ValueError, 'Finish active or queued'),
            ):
                migration.checkpoint_publication(self.options, owner=self.owner)
            self.assertEqual(self.heads(), (self.published, self.legacy))
            self.assertFalse(self.options.output.exists())

    def test_a_matching_retry_bundle_cannot_substitute_a_legacy_json_root(self) -> None:
        def fail(*arguments: str, cwd: Path | None = None) -> str:
            if 'push' in arguments:
                raise subprocess.TimeoutExpired(arguments, 30)
            return self.git(*arguments, cwd=cwd)

        with self.assertRaises(subprocess.TimeoutExpired):
            migration.checkpoint_publication(self.options, owner=replace(self.owner, git=fail))
        journal = self.options.output / 'checkpoint.json'
        report = object_value(read_json(journal))
        bundle = self.options.output / 'checkpoint.bundle'
        self.git('fetch', str(bundle), 'refs/heads/after-store', 'refs/heads/after-state')
        state = self.owner.state(self.checkout, str(report['after_state']), fetch=False)
        forged = self.legacy_commit(state)
        report['after_state'] = forged
        for name, commit in migration.checkpoint_refs(report).items():
            self.git('update-ref', 'refs/heads/' + name, commit)
        substituted = self.options.output / 'substituted.bundle'
        self.git('bundle', 'create', str(substituted), *('refs/heads/' + name for name in migration.REFS))
        substituted.replace(bundle)
        journal.write_bytes(json_bytes(report))
        with self.assertRaisesRegex(ValueError, 'Checkpoint differs'):
            migration.checkpoint_publication(self.options, owner=self.owner)
        self.assertEqual(self.heads(), (self.published, self.legacy))

    def test_unqualified_state_or_stale_expected_heads_cannot_be_checkpointed(self) -> None:
        for status in ('prepared', 'advertised', 'aborted', 'withdrawn'):
            with self.subTest(status=status):
                current = self.legacy_commit({**self.state, 'status': status})
                self.git(
                    'push',
                    '--force-with-lease=refs/heads/unqualified-fixture:',
                    str(self.remote),
                    current + ':refs/heads/unqualified-fixture',
                )
                self.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/hass-publication', current)
                options = replace(self.options, state_commit=current)
                with self.assertRaisesRegex(ValueError, 'current qualified'):
                    migration.checkpoint_publication(options, owner=self.owner)
                self.assertEqual(self.heads(), (self.published, current))
                self.assertFalse(options.output.exists())
                self.git('--git-dir=' + str(self.remote), 'update-ref', '-d', 'refs/heads/unqualified-fixture')
        with self.assertRaisesRegex(ValueError, 'inspect fresh heads'):
            migration.checkpoint_publication(self.options, owner=self.owner)


if __name__ == '__main__':
    unittest.main()
