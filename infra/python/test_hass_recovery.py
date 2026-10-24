"""Real disposable Git/OCI recovery and adversarial isolation of hosted drill targets."""

import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import chdir
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import hass_recovery as recovery
from fixtures.registry import registry
from haos_repository import git as isolated_git
from hass_publish import IMAGE, REMOTE, check_image_version
from hass_recovery_git import StoreDrill
from json_data import Object, json_bytes, object_value, read_json


class RecoveryTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.remote = self.root / 'remote.git'
        self.git('init', '--bare', '-q', str(self.remote))
        self.source = self.root / 'source'
        self.git('init', '-q', str(self.source))
        (self.source / 'README.md').write_text('Synthetic protected public metadata\n')
        self.git('add', '.', cwd=self.source)
        self.git('commit', '-qm', 'Synthetic protected fixture', cwd=self.source)
        self.commit = self.git('rev-parse', 'HEAD', cwd=self.source)
        self.git(
            'push',
            str(self.remote),
            self.commit + ':refs/heads/hass-apps',
            self.commit + ':refs/heads/hass-publication',
            self.commit + ':refs/heads/main',
            self.commit + ':refs/tags/v0.1.2',
            cwd=self.source,
        )
        self.target = recovery.Target('123', '1', self.commit)
        self.evidence = self.root / 'evidence'
        self.evidence.mkdir()
        (self.evidence / 'ownership.json').write_bytes(json_bytes(self.target.identity()))
        (self.evidence / 'refs.json').write_bytes(json_bytes({}))
        self.calls: list[tuple[str, ...]] = []

        def transport(*arguments: str, cwd: Path | None = None) -> str:
            self.calls.append(arguments)
            return self.git(
                *(str(self.remote) if value == REMOTE else value for value in arguments), cwd=cwd or self.source
            )

        self.addCleanup(patch.stopall)
        patch('hass_recovery.git', side_effect=transport).start()
        self.drill = recovery.Drill(self.target, self.evidence)
        self.protected = self.drill.protected_refs()
        (self.evidence / 'protected.json').write_bytes(json_bytes({'refs': self.protected}))
        self.calls.clear()

    def git(self, *arguments: str, cwd: Path | None = None) -> str:
        return isolated_git(self.root, *arguments, cwd=cwd)

    def test_real_git_recovery_and_cleanup_preserve_all_public_refs(self) -> None:
        result = StoreDrill(self.drill.store(), self.evidence / 'store').run()
        self.assertEqual(
            object_value(result['checkpoint']),
            {'history_replaced': True, 'metadata_preserved': True, 'lost_reply_retried': True},
        )
        self.assertEqual(
            object_value(result['withdrawal']),
            {
                'app_removed': True,
                'retry_preserved': True,
                'stale_promotion_rejected': True,
                'replacement_qualified': True,
            },
        )
        for name in (
            'prepared_interruption',
            'delayed_advertisement_rejected',
            'lost_atomic_reply',
            'failed_qualification_rollback',
            'retry_qualified',
            'qualified_retry_preserved',
            'concurrent_metadata_preserved',
            'concurrent_state_preserved',
        ):
            self.assertIs(result[name], True, name)
        self.assertEqual(self.drill.protected_refs(), self.protected)
        with (
            chdir(self.root),
            patch('hass_recovery.api', return_value=None),
            patch.object(recovery.Drill, 'release', return_value=None),
        ):
            self.drill.cleanup()
            self.drill.cleanup()
        self.assertEqual(self.drill.protected_refs(), self.protected)
        self.assertTrue(all(not self.drill.head(ref) for ref in self.target.refs))
        self.assertIs(object_value(read_json(self.evidence / 'cleanup.json'))['passed'], True)
        for arguments in self.calls:
            if 'push' in arguments:
                refs = arguments[arguments.index(REMOTE) + 1 :]
                self.assertTrue(all(value.partition(':')[2] in self.target.refs for value in refs))

    def test_push_guard_rejects_public_refs_unrestricted_options_and_other_remotes(self) -> None:
        reference = self.target.refs[0]
        for arguments in (
            ('push', '--force-with-lease=refs/heads/hass-apps:', REMOTE, self.commit + ':refs/heads/hass-apps'),
            ('push', '--force-with-lease=refs/heads/main:', REMOTE, self.commit + ':refs/heads/main'),
            ('push', '--force-with-lease=refs/tags/v9.0.0:', REMOTE, self.commit + ':refs/tags/v9.0.0'),
            ('push', '--mirror', REMOTE, self.commit + ':' + reference),
            ('push', REMOTE, self.commit + ':' + reference),
            ('push', '--force-with-lease=' + reference + ':', 'https://example.invalid', self.commit + ':' + reference),
            ('push', '--force-with-lease=' + reference + ':', REMOTE, 'HEAD:' + reference),
        ):
            with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                self.drill.execute(*arguments)
        self.assertEqual(self.calls, [], 'rejected pushes cannot reach Git')
        self.assertEqual(object_value(read_json(self.evidence / 'refs.json')), {})

    def test_delayed_advertisement_reaches_the_server_when_its_lease_is_refreshed(self) -> None:
        owner = self.drill.store()
        prepared = self.evidence / 'store/prepared/promotion.json'
        refreshed = False

        def refresh_lease(*arguments: str, cwd: Path | None = None) -> str:
            nonlocal refreshed
            if 'push' in arguments and prepared.is_file():
                journal = object_value(read_json(prepared))
                if str(journal['fence_advertised']) + ':refs/heads/' + owner.fence in arguments:
                    stale = '--force-with-lease=refs/heads/' + owner.fence + ':' + str(journal['fence_prepared'])
                    fresh = '--force-with-lease=refs/heads/' + owner.fence + ':' + owner.head(owner.fence)
                    arguments = tuple(fresh if argument == stale else argument for argument in arguments)
                    refreshed = True
            return owner.git(*arguments, cwd=cwd)

        with self.assertRaisesRegex(RuntimeError, 'delayed advertisement bypassed'):
            StoreDrill(replace(owner, git=refresh_lease), self.evidence / 'store').run()
        self.assertTrue(refreshed)
        journal = object_value(read_json(prepared))
        self.assertEqual(owner.head(owner.branch), journal['published'])
        self.assertEqual(owner.head(owner.fence), journal['fence_advertised'])

    def test_delayed_advertisement_transport_failure_cannot_pass_as_lease_rejection(self) -> None:
        owner = self.drill.store()
        prepared = self.evidence / 'store/prepared/promotion.json'
        failure = subprocess.CalledProcessError(128, ['git', 'push'], stderr='Synthetic transport failure')

        def fail_transport(*arguments: str, cwd: Path | None = None) -> str:
            if 'push' in arguments and prepared.is_file():
                journal = object_value(read_json(prepared))
                if str(journal['fence_advertised']) + ':refs/heads/' + owner.fence in arguments:
                    raise failure
            return owner.git(*arguments, cwd=cwd)

        with self.assertRaises(subprocess.CalledProcessError) as raised:
            StoreDrill(replace(owner, git=fail_transport), self.evidence / 'store').run()
        self.assertIs(raised.exception, failure)

    def test_cleanup_retains_unowned_fixture_updates(self) -> None:
        StoreDrill(self.drill.store(), self.evidence / 'store').run()
        reference = self.target.refs[0]
        self.git('--git-dir=' + str(self.remote), 'update-ref', reference, self.commit)
        before = self.git('ls-remote', '--refs', str(self.remote))
        with (
            patch('hass_recovery.api', return_value=None),
            patch.object(recovery.Drill, 'release', return_value=None),
            self.assertRaisesRegex(ValueError, 'unowned commit'),
        ):
            self.drill.cleanup()
        self.assertEqual(self.git('ls-remote', '--refs', str(self.remote)), before)

    def test_cleanup_refuses_public_or_retargeted_release_and_unrelated_assets(self) -> None:
        tag = self.target.refs[2]
        self.drill.execute('push', '--force-with-lease=' + tag + ':', REMOTE, self.commit + ':' + tag, cwd=self.source)
        releases: tuple[Object, ...] = (
            {'draft': False, 'tag_name': self.target.prefix, 'assets': []},
            {'draft': True, 'tag_name': 'v0.1.2', 'assets': []},
            {'draft': True, 'tag_name': self.target.prefix, 'assets': [{'name': 'unrelated.bin'}]},
        )
        for release in releases:
            with (
                self.subTest(release=release),
                patch.object(recovery.Drill, 'release', return_value=release),
                patch('hass_recovery.command') as command,
                self.assertRaises(ValueError),
            ):
                self.drill.cleanup()
            command.assert_not_called()
        self.git('--git-dir=' + str(self.remote), 'update-ref', '-d', tag)
        with (
            patch.object(
                recovery.Drill, 'release', return_value={'draft': True, 'tag_name': self.target.prefix, 'assets': []}
            ),
            patch('hass_recovery.command') as command,
            self.assertRaises(ValueError),
        ):
            self.drill.cleanup()
        command.assert_not_called()

    def test_draft_discovery_includes_later_pages_and_rejects_ambiguous_or_invalid_responses(self) -> None:
        draft: Object = {'tag_name': self.target.prefix, 'draft': True, 'id': 123}
        with patch('hass_recovery.command', return_value=json.dumps([[{'tag_name': 'v0.1.2'}], [draft]])) as command:
            self.assertEqual(self.drill.release(), draft)
        self.assertIn('--paginate', command.call_args.args)
        self.assertIn('--slurp', command.call_args.args)
        for response in ({'tag_name': self.target.prefix}, [[draft], [draft]], [None]):
            with (
                self.subTest(response=response),
                patch('hass_recovery.command', return_value=json.dumps(response)),
                self.assertRaises(ValueError),
            ):
                self.drill.release()

    def test_targets_reject_non_workflow_identifiers_and_never_name_the_app_package(self) -> None:
        for run, attempt, commit in (
            ('0', '1', self.commit),
            ('123', '../main', self.commit),
            ('123', '1', 'HEAD'),
            ('9' * 21, '1', self.commit),
        ):
            with self.subTest(run=run, attempt=attempt), self.assertRaises(ValueError):
                recovery.Target(run, attempt, commit)
        self.assertNotEqual(self.target.image, IMAGE)
        self.assertTrue(all('hass-recovery-123-1' in ref for ref in self.target.refs))

    def test_cleanup_retains_unowned_package_versions(self) -> None:
        recovery.fixture_image(self.evidence / 'image-baseline', 'baseline')
        recovery.fixture_image(self.evidence / 'image-candidate', 'candidate')
        (self.evidence / 'package-owned.json').write_bytes(json_bytes(self.target.identity()))
        package: Object = {'name': self.target.package, 'repository': {'full_name': recovery.REPOSITORY}}

        def package_api(path: str) -> Object | None:
            return package if '/packages/' in path else None

        with (
            patch('hass_recovery.api', side_effect=package_api),
            patch.object(recovery.Drill, 'release', return_value=None),
            patch('hass_recovery.command', return_value='[{"name":"sha256:unowned"}]') as command,
            self.assertRaisesRegex(ValueError, 'unowned GHCR image'),
        ):
            self.drill.cleanup()
        self.assertTrue(all('DELETE' not in call.args for call in command.call_args_list))

    def test_hosted_failures_preserve_exit_codes_and_remove_registry_credentials(self) -> None:
        environment = {
            'GITHUB_ACTIONS': 'true',
            'RUNNER_ENVIRONMENT': 'github-hosted',
            'RUNNER_OS': 'Linux',
            'GITHUB_REPOSITORY': recovery.REPOSITORY,
            'GITHUB_SHA': self.commit,
            'GITHUB_RUN_ID': '123',
            'GITHUB_RUN_ATTEMPT': '1',
            'GITHUB_ACTOR': 'synthetic',
            'GH_TOKEN': 'synthetic fixture credential',
        }
        failures = (
            (subprocess.TimeoutExpired(['skopeo'], 60), 124),
            (subprocess.CalledProcessError(-15, ['skopeo'], stderr=b'interrupted fixture command'), 143),
        )
        for index, (failure, expected) in enumerate(failures):
            auth = self.root / str(index) / 'credentials/config.json'

            def login(*_args: object, destination: Path = auth, **_kwargs: object) -> None:
                destination.write_text('{}')

            with (
                self.subTest(expected=expected),
                patch.dict(os.environ, environment),
                patch('hass_recovery.AUTH', auth),
                patch('hass_recovery.OUTPUT'),
                patch('hass_recovery.git', return_value=self.commit),
                patch('hass_recovery.subprocess.run', side_effect=login),
                patch.object(recovery.Drill, 'run', side_effect=failure),
            ):
                self.assertEqual(recovery.main(recovery.Options('run')), expected)
            self.assertFalse(auth.exists())
        with (
            patch.dict(os.environ, {'GITHUB_ACTIONS': 'false'}),
            patch('hass_recovery.OUTPUT'),
            patch('hass_recovery.command') as command,
        ):
            self.assertEqual(recovery.main(recovery.Options('run')), 1)
        command.assert_not_called()

    def test_tiny_oci_fixture_round_trips_and_conflicting_versions_are_rejected(self) -> None:
        original = recovery.fixture_image(self.root / 'original', 'original')
        replacement = recovery.fixture_image(self.root / 'replacement', 'replacement')
        with registry(self.root) as image:

            def digest(version: str) -> str:
                data = subprocess.run(
                    ['skopeo', 'inspect', '--raw', '--tls-verify=false', 'docker://' + image + ':' + version],
                    check=True,
                    capture_output=True,
                    timeout=30,
                ).stdout
                return 'sha256:' + hashlib.sha256(data).hexdigest()

            subprocess.run(
                [
                    'skopeo',
                    # Only these unsigned local fixtures bypass host signature policy.
                    '--insecure-policy',
                    'copy',
                    '--all',
                    '--preserve-digests',
                    '--dest-no-creds',
                    '--dest-tls-verify=false',
                    'oci:' + str(self.root / 'original'),
                    'docker://' + image + ':0.0.1',
                ],
                check=True,
                capture_output=True,
                timeout=30,
            )
            self.assertEqual(digest('0.0.1'), original)
            with self.assertRaisesRegex(ValueError, 'refusing to replace'):
                check_image_version('0.0.1', replacement, {'0.0.1'}, digest)
            check_image_version('0.0.1', original, {'0.0.1'}, digest)
            check_image_version('0.0.2', replacement, {'0.0.1'}, digest)
            self.assertEqual(digest('0.0.1'), original)
            subprocess.run(
                [
                    'skopeo',
                    '--insecure-policy',
                    'copy',
                    '--all',
                    '--preserve-digests',
                    '--dest-no-creds',
                    '--dest-tls-verify=false',
                    'oci:' + str(self.root / 'replacement'),
                    'docker://' + image + ':0.0.2',
                ],
                check=True,
                capture_output=True,
                timeout=30,
            )
            self.assertEqual(digest('0.0.2'), replacement)


if __name__ == '__main__':
    unittest.main()
