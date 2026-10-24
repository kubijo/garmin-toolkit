"""Withdraw synthetic releases through real, disposable Git refs without touching published artifacts."""

import json
import shutil
import subprocess
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import hass_publish as publication
from hass_release_contract import Release
from hass_store import FENCE, STATE_FILE, Store
from json_data import Object, object_value, read_json


class WithdrawalTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix='hass-withdrawal-test.')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.remote = self.root / 'remote.git'
        subprocess.run(['git', 'init', '--bare', '-q', str(self.remote)], check=True)
        self.addCleanup(patch.stopall)
        patch.object(publication, 'REMOTE', str(self.remote)).start()
        real_command = publication.command

        def git_only(*args: str, cwd: Path | None = None) -> str:
            if args[0] != 'git':
                raise AssertionError('Withdrawal must preserve release assets and registry bytes: ' + str(args))
            return real_command(*args, cwd=cwd)

        patch.object(publication, 'command', side_effect=git_only).start()
        self.repository = self.root / 'repository'
        app = self.repository / 'garmin_toolkit'
        app.mkdir(parents=True)
        (self.repository / 'repository.yaml').write_text('name: Synthetic app repository\n')
        (self.repository / 'README.md').write_text('Preserved root documentation\n')
        config = app / 'config.yaml'
        config.write_text('slug: garmin_toolkit\nversion: 0.0.1\n')
        self.previous = publication.push_metadata(self.repository, 'hass-apps', '')
        config.write_text('slug: garmin_toolkit\nversion: 0.1.0\n')
        self.promotion = self.root / 'promotion'
        self.promotion.mkdir()
        self.published = publication.push_metadata(
            self.repository,
            'hass-apps',
            self.previous,
            before_push=lambda commit, current, checkout: publication.store().prepare(
                checkout, self.promotion, current, commit
            ),
        )
        self.unqualified = self.root / 'unqualified'
        shutil.copytree(self.promotion, self.unqualified)
        publication.qualify_promotion(self.promotion)
        self.fence = publication.branch_head(FENCE)
        self.options = publication.WithdrawPublication(
            version='0.1.0',
            store_commit=self.published,
            state_commit=self.fence,
            reason='Synthetic installation failure',
            output=self.root / 'withdrawal',
        )

    def state(self) -> Object:
        return object_value(
            json.loads(
                publication.git(
                    '--git-dir=' + str(self.remote), 'show', publication.branch_head(FENCE) + ':' + STATE_FILE
                )
            )
        )

    def test_withdrawal_removes_only_the_app_and_retains_both_histories(self) -> None:
        publication.withdraw_publication(self.options)
        withdrawn = publication.branch_head('hass-apps')
        self.assertNotEqual(withdrawn, self.published)
        files = publication.git('--git-dir=' + str(self.remote), 'ls-tree', '-r', '--name-only', withdrawn)
        self.assertEqual(files.splitlines(), ['README.md', 'repository.yaml'])
        for name in files.splitlines():
            self.assertEqual(
                publication.git('--git-dir=' + str(self.remote), 'rev-parse', withdrawn + ':' + name),
                publication.git('--git-dir=' + str(self.remote), 'rev-parse', self.published + ':' + name),
            )
        self.assertEqual(
            publication.git('--git-dir=' + str(self.remote), 'show', '-s', '--format=%P', withdrawn), self.published
        )
        self.assertEqual(
            publication.git(
                '--git-dir=' + str(self.remote), 'show', '-s', '--format=%P', publication.branch_head(FENCE)
            ),
            self.fence,
        )
        self.assertEqual(self.state()['status'], 'withdrawn')
        self.assertEqual(self.state()['published'], withdrawn)
        self.assertEqual(self.state()['previous'], self.published)
        self.assertEqual(self.state()['reason'], self.options.reason)
        self.assertIn(
            'version: 0.1.0',
            publication.git('--git-dir=' + str(self.remote), 'show', self.published + ':garmin_toolkit/config.yaml'),
        )
        self.assertIs(object_value(read_json(self.options.output / 'withdrawal.json'))['completed'], True)
        fence = publication.branch_head(FENCE)
        publication.withdraw_publication(replace(self.options, output=self.root / 'retry'))
        self.assertEqual(publication.branch_head('hass-apps'), withdrawn)
        self.assertEqual(publication.branch_head(FENCE), fence)
        with self.assertRaisesRegex(ValueError, 'changed'):
            publication.withdraw_publication(
                replace(self.options, reason='Different request', output=self.root / 'changed')
            )
        self.assertEqual(publication.branch_head('hass-apps'), withdrawn)
        self.assertEqual(publication.branch_head(FENCE), fence)

    def test_failure_before_push_preserves_both_refs_and_allows_a_fresh_retry(self) -> None:
        with (
            patch.object(Store, 'push', side_effect=subprocess.TimeoutExpired('synthetic unsent push', 630)),
            self.assertRaises(subprocess.TimeoutExpired),
        ):
            publication.withdraw_publication(self.options)
        self.assertEqual(publication.branch_head('hass-apps'), self.published)
        self.assertEqual(publication.branch_head(FENCE), self.fence)
        self.assertIs(object_value(read_json(self.options.output / 'withdrawal.json'))['completed'], False)
        publication.withdraw_publication(replace(self.options, output=self.root / 'retry'))
        self.assertEqual(self.state()['status'], 'withdrawn')

    def test_ambiguous_push_can_be_retried_without_replacing_the_original_journal(self) -> None:
        real_push = Store.push

        def lost_reply(
            owner: Store, checkout: Path, before: str, after: str, *, advertisement: tuple[str, str] | None = None
        ) -> None:
            real_push(owner, checkout, before, after, advertisement=advertisement)
            raise subprocess.TimeoutExpired('synthetic lost push response', 630)

        with patch.object(Store, 'push', new=lost_reply), self.assertRaises(subprocess.TimeoutExpired):
            publication.withdraw_publication(self.options)
        self.assertEqual(self.state()['status'], 'withdrawn')
        self.assertIs(object_value(read_json(self.options.output / 'withdrawal.json'))['completed'], False)
        with self.assertRaisesRegex(ValueError, 'retain its original journal'):
            publication.withdraw_publication(self.options)
        publication.withdraw_publication(replace(self.options, output=self.root / 'retry'))
        self.assertIs(object_value(read_json(self.root / 'retry/withdrawal.json'))['completed'], True)

    def test_invalid_identity_or_unfinished_state_cannot_change_either_ref(self) -> None:
        for options in (
            replace(self.options, version='0.2.0'),
            replace(self.options, version='v0.1.0'),
            replace(self.options, reason='   '),
            replace(self.options, store_commit=self.previous),
            replace(self.options, state_commit=''),
        ):
            with self.subTest(options=options), self.assertRaises(ValueError):
                publication.withdraw_publication(options)
            self.assertEqual(publication.branch_head('hass-apps'), self.published)
            self.assertEqual(publication.branch_head(FENCE), self.fence)
        publication.git('--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/' + FENCE, self.state_parent())
        with self.assertRaisesRegex(ValueError, 'durably qualified'):
            publication.withdraw_publication(replace(self.options, state_commit=publication.branch_head(FENCE)))
        self.assertEqual(publication.branch_head('hass-apps'), self.published)

    def state_parent(self) -> str:
        return publication.git('--git-dir=' + str(self.remote), 'show', '-s', '--format=%P', self.fence)

    def test_concurrent_change_rejects_the_whole_atomic_push(self) -> None:
        real_push = Store.push
        for branch in ('hass-apps', FENCE):
            with self.subTest(branch=branch):
                before = publication.branch_head(branch)

                def changed(
                    owner: Store,
                    checkout: Path,
                    old: str,
                    new: str,
                    *,
                    advertisement: tuple[str, str] | None = None,
                    target: str = branch,
                    expected: str = before,
                ) -> None:
                    publication.git(
                        '--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/' + target, self.previous, expected
                    )
                    real_push(owner, checkout, old, new, advertisement=advertisement)

                with patch.object(Store, 'push', new=changed), self.assertRaises(subprocess.CalledProcessError):
                    publication.withdraw_publication(replace(self.options, output=self.root / branch))
                self.assertEqual(publication.branch_head(branch), self.previous)
                self.assertEqual(
                    publication.branch_head(FENCE if branch == 'hass-apps' else 'hass-apps'),
                    self.fence if branch == 'hass-apps' else self.published,
                )
                publication.git(
                    '--git-dir=' + str(self.remote), 'update-ref', 'refs/heads/' + branch, before, self.previous
                )

    def test_stale_promotion_and_rollback_preserve_withdrawal_but_a_new_release_can_advance(self) -> None:
        publication.withdraw_publication(self.options)
        withdrawn, fence = publication.branch_head('hass-apps'), publication.branch_head(FENCE)
        with self.assertRaisesRegex(ValueError, 'unrelated recovery'):
            publication.rollback_store(publication.RollbackStore(directory=self.unqualified))
        publication.store().reconcile(self.root)
        document: Release = {
            'schema_version': 1,
            'status': 'candidate',
            'version': '0.1.0',
            'platforms': [],
            'files': [],
            'acceptance': [],
            'source': {'repository': publication.REMOTE, 'commit': 'a' * 40, 'tag': 'v0.1.0', 'lock_sha256': 'b' * 64},
            'image': {'repository': publication.IMAGE, 'index_digest': 'sha256:' + 'c' * 64},
        }
        real_command = publication.command

        def reads_only(*args: str, cwd: Path | None = None) -> str:
            return '{"Tags":[]}' if args[:2] == ('skopeo', 'list-tags') else real_command(*args, cwd=cwd)

        with (
            patch.object(publication, 'assert_remote_tag'),
            patch.object(publication, 'command', side_effect=reads_only),
            self.assertRaisesRegex(ValueError, 'unrelated metadata'),
        ):
            publication.promote_candidate(
                publication.StagePublication(source=self.root, directory=self.root, build=self.root),
                document,
                {'previous': self.previous},
            )
        self.assertEqual(publication.branch_head('hass-apps'), withdrawn)
        self.assertEqual(publication.branch_head(FENCE), fence)
        (self.repository / 'garmin_toolkit/config.yaml').write_text('slug: garmin_toolkit\nversion: 0.1.1\n')
        new = self.root / 'new-promotion'
        new.mkdir()
        published = publication.push_metadata(
            self.repository,
            'hass-apps',
            withdrawn,
            before_push=lambda commit, current, checkout: publication.store().prepare(checkout, new, current, commit),
        )
        publication.qualify_promotion(new)
        self.assertEqual(publication.branch_head('hass-apps'), published)
        self.assertEqual(self.state()['status'], 'qualified')


if __name__ == '__main__':
    unittest.main()
