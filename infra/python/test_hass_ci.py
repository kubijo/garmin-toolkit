"""CI orchestration failures must preserve release gates, rollback, and secret cleanup."""

import io
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import ExitStack, redirect_stdout, suppress
from copy import deepcopy
from itertools import product
from multiprocessing import get_context
from pathlib import Path
from unittest.mock import patch

import tyro

import haos_app_vm as vm
import hass_ci as ci
import hass_publish as publication
import hass_store
from fixtures.images import write_image
from hass_registry import bind_acceptance
from hass_release_contract import Release
from json_data import Object, file_sha256, json_bytes, object_value, read_json

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / 'infra/ci/runner.sh'


def user_manager_available() -> bool:
    try:
        return (
            subprocess.run(
                ['systemctl', '--user', '--no-ask-password', 'show-environment'],
                check=False,
                capture_output=True,
                timeout=5,
            ).returncode
            == 0
        )
    except OSError, subprocess.TimeoutExpired:
        return False


USER_MANAGER = user_manager_available()


def delivery_inputs() -> tuple[Object, Object]:
    return (
        {
            'id': 456,
            'path': '.github/workflows/ci.yml',
            'event': 'pull_request',
            'status': 'completed',
            'conclusion': 'success',
            'head_sha': 'a' * 40,
            'repository': {'full_name': 'kubijo/garmin-toolkit'},
            'head_repository': {'full_name': 'kubijo/garmin-toolkit'},
        },
        {
            'version': '0.1.2',
            'source': {'commit': 'b' * 40},
            'platform': {
                'architecture': 'aarch64',
                'archive_sha256': 'c' * 64,
                'manifest_digest': 'sha256:' + 'd' * 64,
                'config_digest': 'sha256:' + 'e' * 64,
                'workflow_run': 'https://github.com/kubijo/garmin-toolkit/actions/runs/456',
            },
        },
    )


def promotion_worker(root: Path, mode: str, previous: str) -> None:
    ci.PUBLICATION = root / ('publication-' + mode)
    digest = 'sha256:' + 'b' * 64
    document: Release = {
        'schema_version': 1,
        'status': 'candidate',
        'version': '0.1.0',
        'platforms': [],
        'files': [],
        'acceptance': [],
        'source': {'repository': publication.REMOTE, 'commit': 'a' * 40, 'tag': 'v0.1.0', 'lock_sha256': 'c' * 64},
        'image': {'repository': publication.IMAGE, 'index_digest': digest},
    }
    real_command, signalled = publication.command, False
    real_journal = hass_store.write_journal

    def journal(directory: Path, data: Object, *, replace: bool = False) -> None:
        if replace and mode == 'qualify-journal-fail':
            raise RuntimeError('completion journal write failed after remote qualification')
        real_journal(directory, data, replace=replace)

    def interrupt() -> None:
        nonlocal signalled
        if not signalled:
            signalled = True
            os.kill(os.getpid(), signal.SIGKILL if mode.startswith('kill-') else signal.SIGTERM)

    def command(*args: str, cwd: Path | None = None) -> str:
        if args[0] == 'git':
            if mode == 'inflight' and 'push' in args and any(arg.endswith(':refs/heads/hass-apps') for arg in args):
                position = args.index('push') + 1
                args = (*args[:position], '--receive-pack=' + str(root / 'receive-pack'), *args[position:])
            result = real_command(*args, cwd=cwd)
            if mode.endswith('-push') and 'push' in args and any(arg.endswith(':refs/heads/hass-apps') for arg in args):
                interrupt()
            return result
        if args[:2] == ('skopeo', 'list-tags'):
            return '{"Tags":["0.1.0"]}'
        if args[0] == 'skopeo' and 'copy' in args and mode == 'retry-copy-fail':
            raise subprocess.CalledProcessError(1, args)
        if args[0] in ('skopeo', 'gh'):
            return ''
        raise AssertionError('Unexpected external action: ' + str(args))

    def promote(_options: object) -> None:
        ci.PUBLICATION.mkdir()
        shutil.copytree(root / 'metadata', ci.PUBLICATION / 'repository')
        staging: Object = {
            'previous': previous,
            'branch': 'hass-candidate-123-1',
            'commit': 'a' * 40,
            'index_digest': digest,
        }
        (ci.PUBLICATION / 'staging.json').write_bytes(json_bytes(staging))
        publication.promote_candidate(
            publication.StagePublication(source=root, directory=ci.PUBLICATION, build=root), document, staging
        )

    def accept(*_args: object, **_kwargs: object) -> None:
        if mode == 'child-accept':
            ci.run_owned(sys.executable, str(root / 'acceptance.py'))
        if mode.endswith('-accept'):
            interrupt()
        if mode == 'retry-accept-fail':
            raise RuntimeError('simulated qualification failure')

    with (
        patch.object(ci, 'release_guard'),
        patch.object(ci, 'login'),
        patch.object(ci, 'promote_draft', side_effect=promote),
        patch.object(ci, 'acceptance', side_effect=accept),
        patch.object(publication, 'assert_remote_tag'),
        patch.object(publication, 'REMOTE', str(root / 'remote.git')),
        patch.object(publication, 'command', side_effect=command),
        patch.object(publication, 'image_digest', return_value=digest),
        patch.object(hass_store, 'write_journal', side_effect=journal),
    ):
        try:
            ci.promote()
        except RuntimeError, subprocess.CalledProcessError, ValueError:
            raise SystemExit(1) from None


class PhaseTests(unittest.TestCase):
    def test_progress_is_live_and_timing_stays_outside_actions_group(self) -> None:
        for actions in ('true', 'false'):
            with (
                self.subTest(actions=actions),
                patch.dict(os.environ, GITHUB_ACTIONS=actions),
                patch.object(ci, 'monotonic', side_effect=(100, 102.5)),
                redirect_stdout(io.StringIO()) as output,
            ):
                with ci.phase('Export dependency sources'):
                    self.assertIn('Export dependency sources', output.getvalue())
                    self.assertNotIn('passed', output.getvalue())
                    print('source output')
                log = output.getvalue()
                self.assertTrue(log.endswith('Export dependency sources: passed (2.5s)\n'))
                self.assertIn('source output\n', log)
                if actions == 'true':
                    self.assertTrue(log.startswith('::group::Export dependency sources\n'))
                    self.assertIn('::endgroup::\nExport dependency sources: passed', log)
                else:
                    self.assertNotIn('::', log)

    def test_failure_and_cancellation_close_groups_and_preserve_the_exception(self) -> None:
        for failure in (RuntimeError('export failed'), KeyboardInterrupt(), SystemExit(143)):
            with (
                self.subTest(failure=type(failure).__name__),
                patch.dict(os.environ, GITHUB_ACTIONS='true'),
                patch.object(ci, 'monotonic', side_effect=(100, 103)),
                redirect_stdout(io.StringIO()) as output,
            ):
                with self.assertRaises(type(failure)) as caught, ci.phase('Export dependency sources'):
                    raise failure
                self.assertIs(caught.exception, failure)
                self.assertTrue(output.getvalue().endswith('::endgroup::\nExport dependency sources: failed (3.0s)\n'))
                self.assertNotIn('passed', output.getvalue())

    def test_closed_output_cannot_skip_work_or_hide_errors(self) -> None:
        for output_error in (BrokenPipeError('pipe closed'), ValueError('stream closed')):
            with (
                self.subTest(output_error=type(output_error).__name__),
                patch.dict(os.environ, GITHUB_ACTIONS='true'),
                patch('builtins.print', side_effect=output_error),
            ):
                work: list[str] = []
                with ci.phase('Export dependency sources'):
                    work.append('exported')
                self.assertEqual(work, ['exported'])
                for failure in (RuntimeError('export failed'), KeyboardInterrupt(), SystemExit(143)):
                    with self.assertRaises(type(failure)) as caught, ci.phase('Export dependency sources'):
                        raise failure
                    self.assertIs(caught.exception, failure)


class CITests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(dir=ROOT / '.tmp')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        previous = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, previous)
        self.scratch = self.root / '.tmp'
        self.scratch.mkdir()
        self.addCleanup(patch.stopall)
        patch.dict(
            os.environ,
            {
                'GITHUB_OUTPUT': str(self.root / 'outputs'),
                'GITHUB_REPOSITORY': 'kubijo/garmin-toolkit',
                'GITHUB_RUN_ID': '123',
                'DOCKER_CONFIG': str(self.scratch / 'docker-auth'),
                'REGISTRY_AUTH_FILE': str(self.scratch / 'docker-auth/config.json'),
                'GITHUB_ACTOR': 'test-actor',
                'GH_TOKEN': 'synthetic-secret',
                'GITHUB_ACTIONS': 'true',
                'RUNNER_ENVIRONMENT': 'github-hosted',
                'RUNNER_OS': 'Linux',
            },
        ).start()
        self.home = self.root / 'home'
        self.home.mkdir()
        patch.object(Path, 'home', return_value=self.home).start()

    def test_cli_and_native_source_selection(self) -> None:
        for system, sources, release, expected in (
            ('x86_64-linux', True, False, True),
            ('aarch64-linux', True, False, True),
            ('x86_64-linux', False, False, False),
            ('x86_64-linux', False, True, True),
        ):
            with self.subTest(system=system, sources=sources, release=release):
                options = tyro.cli(
                    ci.Options,
                    args=[
                        'native',
                        '--system',
                        system,
                        '--sources',
                        str(sources).lower(),
                        '--release',
                        str(release).lower(),
                    ],
                )
                with (
                    patch.object(ci, 'release_guard') as guard,
                    patch.object(ci, 'build', return_value=Path('/nix/store/synthetic')),
                    patch.object(ci, 'run', return_value='{}'),
                    patch.object(ci, 'record_build') as record,
                    patch.object(ci, 'bundle_sources') as bundle,
                ):
                    ci.native(options)
                self.assertEqual(bundle.called, expected)
                self.assertEqual(guard.called, release)
                self.assertEqual(
                    record.call_args.args[0].workflow_run, 'https://github.com/kubijo/garmin-toolkit/actions/runs/123'
                )
                self.assertIn(f'sources={str(expected).lower()}\n', (self.root / 'outputs').read_text())
                (self.root / 'outputs').unlink()

    def test_release_guard_stops_build_and_login(self) -> None:
        with (
            patch.object(ci, 'release_guard', side_effect=ValueError('wrong tag')),
            patch.object(ci, 'build') as build,
            patch.object(ci, 'login') as login,
        ):
            for command in (lambda: ci.native(ci.Options(command='native', release='true')), ci.draft, ci.promote):
                with self.assertRaisesRegex(ValueError, 'wrong tag'):
                    command()
            build.assert_not_called()
            login.assert_not_called()

    def test_source_failure_retains_verified_native_outputs_but_image_failure_does_not(self) -> None:
        for failed in ('record_build', 'bundle_sources'):
            with (
                self.subTest(failed=failed),
                patch.object(ci, 'build', return_value=Path('/nix/store/synthetic')),
                patch.object(ci, 'run', return_value='{}'),
                patch.object(ci, 'record_build'),
                patch.object(ci, 'bundle_sources'),
                patch.object(ci, failed, side_effect=ValueError('export failed')),
                self.assertRaisesRegex(ValueError, 'export failed'),
            ):
                ci.native(ci.Options(command='native', sources='true'))
            output = self.root / 'outputs'
            if failed == 'record_build':
                self.assertFalse(output.exists())
            else:
                self.assertEqual(output.read_text(), 'architecture=amd64\nsources=true\n')

    def test_login_keeps_token_off_command_line_and_uses_private_storage(self) -> None:
        with patch.object(ci.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run:
            ci.login()
        self.assertEqual(run.call_args.kwargs['input'], 'synthetic-secret')
        self.assertNotIn('synthetic-secret', run.call_args.args[0])
        self.assertIn('--password-stdin', run.call_args.args[0])
        self.assertEqual((self.scratch / 'docker-auth').stat().st_mode & 0o777, 0o700)
        with self.assertRaisesRegex(ValueError, 'fresh owned'):
            ci.login()

    def test_login_exposes_the_same_credentials_at_the_attestation_readers_path_and_restores_original(self) -> None:
        default = self.home / '.docker/config.json'
        default.parent.mkdir()
        original = b'{"auths":{"other.example":{"auth":"preserved"}}}\n'
        default.write_bytes(original)
        previous = default.stat()
        credentials = self.scratch / 'docker-auth/config.json'

        def logged_in(*_args: str, **_kwargs: object) -> str:
            credentials.write_bytes(json_bytes({'auths': {'ghcr.io': {'auth': 'synthetic'}}}))
            return ''

        with patch.object(ci, 'run', side_effect=logged_in):
            ci.login()
        self.assertTrue(default.samefile(credentials))
        self.assertEqual(default.read_bytes(), credentials.read_bytes())
        with patch.object(ci, 'cleanup_staging') as recover:
            ci.cleanup()
        self.assertFalse(credentials.exists())
        self.assertFalse(default.is_symlink())
        self.assertEqual(default.read_bytes(), original)
        self.assertEqual(default.stat().st_ino, previous.st_ino)
        recover.assert_called_once()
        with patch.object(ci, 'cleanup_staging'):
            ci.cleanup()
        self.assertEqual(default.read_bytes(), original)

    def test_login_restores_an_original_symlink_and_removes_only_its_own_link(self) -> None:
        default = self.home / '.docker/config.json'
        default.parent.mkdir()
        original = self.home / 'original-config'
        original.write_text('original credentials')
        default.symlink_to(original)
        with patch.object(ci, 'run'):
            ci.login()
        with (
            patch.object(ci, 'cleanup_staging', side_effect=RuntimeError('recovery failed')),
            self.assertRaisesRegex(RuntimeError, 'recovery failed'),
        ):
            ci.cleanup()
        self.assertTrue(default.is_symlink())
        self.assertEqual(default.readlink(), original)
        self.assertEqual(default.read_text(), 'original credentials')

    def test_registry_credentials_refuse_local_runners_before_changing_files_or_sending_token(self) -> None:
        for name, value in (
            ('GITHUB_ACTIONS', 'false'),
            ('RUNNER_ENVIRONMENT', 'self-hosted'),
            ('RUNNER_OS', 'macOS'),
        ):
            with (
                self.subTest(name=name),
                patch.dict(os.environ, {name: value}),
                patch.object(ci, 'run') as run,
                patch.object(ci, 'cleanup_staging') as recover,
            ):
                for action in (ci.login, ci.cleanup):
                    with self.assertRaisesRegex(ValueError, 'disposable GitHub-hosted'):
                        action()
            run.assert_not_called()
            recover.assert_not_called()
            self.assertFalse((self.scratch / 'docker-auth').exists())
            self.assertEqual(list(self.home.iterdir()), [])

    def test_login_link_failure_restores_original_config(self) -> None:
        default = self.home / '.docker/config.json'
        default.parent.mkdir()
        default.write_text('original credentials')
        with (
            patch.object(ci, 'run'),
            patch.object(Path, 'symlink_to', side_effect=OSError('link failed')),
            self.assertRaisesRegex(OSError, 'link failed'),
        ):
            ci.login()
        self.assertFalse(default.is_symlink())
        self.assertEqual(default.read_text(), 'original credentials')

    def test_cleanup_without_an_original_config_removes_the_attestation_link(self) -> None:
        with patch.object(ci, 'run'):
            ci.login()
        default = self.home / '.docker/config.json'
        self.assertTrue(default.is_symlink())
        with patch.object(ci, 'cleanup_staging'):
            ci.main(tyro.cli(ci.Options, args=['cleanup']))
        self.assertFalse(default.exists() or default.is_symlink())

    def test_cleanup_retries_config_restoration_without_blocking_publication_recovery(self) -> None:
        default = self.home / '.docker/config.json'
        default.parent.mkdir()
        default.write_text('original credentials')
        with patch.object(ci, 'run'):
            ci.login()
        credentials = self.scratch / 'docker-auth/config.json'
        credentials.write_text('temporary credentials')
        backup = credentials.with_name('original-config')
        with patch.object(ci, 'cleanup_staging') as recover:
            with (
                patch.object(ci.shutil, 'move', side_effect=PermissionError('restore failed')),
                self.assertRaisesRegex(PermissionError, 'restore failed'),
            ):
                ci.cleanup()
            recover.assert_called_once()
            self.assertFalse(credentials.exists())
            self.assertFalse(default.exists() or default.is_symlink())
            self.assertEqual(backup.read_text(), 'original credentials')
            ci.cleanup()
            self.assertEqual(recover.call_count, 2)
        self.assertEqual(default.read_text(), 'original credentials')
        self.assertFalse(backup.exists())

    def test_cleanup_preserves_an_unrelated_config_while_restoration_is_pending(self) -> None:
        default = self.home / '.docker/config.json'
        default.parent.mkdir()
        default.write_text('original credentials')
        with patch.object(ci, 'run'):
            ci.login()
        default.unlink()
        default.symlink_to(self.home / 'unrelated-missing-config')
        with patch.object(ci, 'cleanup_staging') as recover:
            ci.cleanup()
        recover.assert_called_once()
        self.assertEqual(default.readlink(), self.home / 'unrelated-missing-config')
        self.assertEqual((self.scratch / 'docker-auth/original-config').read_text(), 'original credentials')

    def test_credential_removal_failure_still_attempts_publication_recovery(self) -> None:
        with (
            patch.object(Path, 'unlink', side_effect=PermissionError('credentials locked')),
            patch.object(ci, 'cleanup_staging') as recover,
            self.assertRaisesRegex(PermissionError, 'credentials locked'),
        ):
            ci.cleanup()
        recover.assert_called_once()

    def test_draft_qualifies_both_paths_before_exposing_signing_subjects(self) -> None:
        candidate = ci.PUBLICATION / 'candidate'
        candidate.mkdir(parents=True)
        index, manifest = 'sha256:' + 'a' * 64, 'sha256:' + 'b' * 64
        (ci.PUBLICATION / 'staging.json').write_bytes(
            json_bytes(
                {
                    'repository_url': 'https://github.com/kubijo/garmin-toolkit#hass-candidate-123-1',
                    'index_digest': index,
                    'tag': '0.1.0-candidate.123.1',
                }
            )
        )
        (candidate / 'release.json').write_bytes(
            json_bytes(
                {
                    'image': {'index_digest': index},
                    'platforms': [
                        {'architecture': 'amd64', 'manifest_digest': manifest},
                        {'architecture': 'aarch64', 'manifest_digest': 'sha256:' + 'c' * 64},
                    ],
                }
            )
        )

        def acceptance(_: Path, evidence: Path, *arguments: str, **_kwargs: object) -> None:
            evidence.mkdir(parents=True)
            for name in ('workflow.json', 'installation.json'):
                (evidence / name).write_text('synthetic evidence')

        with (
            patch.object(ci, 'release_guard'),
            patch.object(ci, 'build', return_value=Path('/repository')),
            patch.object(ci, 'prepare_publication'),
            patch.object(ci, 'login'),
            patch.object(ci, 'stage_publication'),
            patch.object(
                ci,
                'download_upgrade_baseline',
                side_effect=[Path('/published-baseline/amd64'), Path('/published-baseline/aarch64')],
            ),
            patch.object(ci, 'acceptance', side_effect=acceptance) as check,
            patch.object(ci, 'package_evidence') as package,
        ):
            ci.draft()
        self.assertEqual(check.call_count, 4)
        self.assertIn('--lifecycle', check.call_args_list[0].args)
        self.assertIn('--baseline-release', check.call_args_list[0].args)
        self.assertIn('/published-baseline/amd64', check.call_args_list[0].args)
        self.assertIn('0.1.0-candidate.123.1', check.call_args_list[1].args)
        self.assertIn(index, check.call_args_list[1].args)
        self.assertEqual(
            (self.scratch / 'staged-acceptance/amd64/lifecycle-workflow.json').read_text(), 'synthetic evidence'
        )
        self.assertIn('--baseline-release', check.call_args_list[2].args)
        self.assertIn('/published-baseline/aarch64', check.call_args_list[2].args)
        self.assertIn('aarch64', check.call_args_list[2].args)
        self.assertEqual(check.call_args_list[2].kwargs['timeout'], 1900)
        self.assertIn('--timeout', check.call_args_list[2].args)
        package.assert_called_once()
        self.assertIn(f'index={index}\n', (self.root / 'outputs').read_text())
        self.assertIn(
            'manifest_bundle=.tmp/bundles/sha256-' + 'b' * 64 + '.sigstore.jsonl\n', (self.root / 'outputs').read_text()
        )

    def test_login_refuses_mismatched_authfile_before_sending_token(self) -> None:
        with patch.dict(os.environ, REGISTRY_AUTH_FILE=str(self.root / 'unrelated')):
            with patch.object(ci, 'run') as run, self.assertRaisesRegex(ValueError, 'share'):
                ci.login()
            run.assert_not_called()

    def test_branch_preflight_uses_both_haos_checks_without_release_admission_or_publication(self) -> None:
        candidate = ci.PUBLICATION / 'candidate'
        candidate.mkdir(parents=True)
        index, manifest = 'sha256:' + 'a' * 64, 'sha256:' + 'b' * 64
        document = {
            'version': '0.1.1',
            'image': {'index_digest': index},
            'platforms': [
                {'architecture': 'amd64', 'manifest_digest': manifest},
                {'architecture': 'aarch64', 'manifest_digest': 'sha256:' + 'c' * 64},
            ],
        }
        (candidate / 'registry.json').write_bytes(json_bytes(document))
        (ci.PUBLICATION / 'staging.json').write_bytes(
            json_bytes(
                {
                    'repository_url': 'https://github.com/kubijo/garmin-toolkit#hass-candidate-123-1',
                    'index_digest': index,
                    'tag': '0.1.1-candidate.123.1',
                }
            )
        )

        def acceptance(_: Path, evidence: Path, *arguments: str, **_kwargs: object) -> None:
            evidence.mkdir(parents=True)
            for name in ('workflow.json', 'installation.json'):
                (evidence / name).write_text('Synthetic branch evidence\n')

        with (
            patch.object(ci, 'release_guard') as guard,
            patch.object(ci, 'prepare_publication') as prepare,
            patch.object(ci, 'stage_publication') as stage,
            patch.object(ci, 'build', return_value=Path('/repository')),
            patch.object(ci, 'login'),
            patch.object(ci.hass_preflight, 'guard'),
            patch.object(ci.hass_preflight, 'prepare'),
            patch.object(ci.hass_preflight, 'check_candidate', return_value=(document, document)),
            patch.object(ci.hass_preflight, 'package') as package,
            patch.object(ci, 'stage_image') as image,
            patch.object(
                ci,
                'download_upgrade_baseline',
                side_effect=[Path('/published-baseline/amd64'), Path('/published-baseline/aarch64')],
            ),
            patch.object(ci, 'acceptance', side_effect=acceptance) as check,
        ):
            ci.main(tyro.cli(ci.Options, args=['preflight']))
        guard.assert_not_called()
        prepare.assert_not_called()
        stage.assert_not_called()
        package.assert_called_once()
        image.assert_called_once()
        self.assertEqual(image.call_args.args[1:], ('0.1.1', index, ''))
        self.assertEqual(check.call_count, 4)
        self.assertIn('--lifecycle', check.call_args_list[0].args)
        self.assertIn('--baseline-release', check.call_args_list[0].args)
        self.assertIn('/published-baseline/amd64', check.call_args_list[0].args)
        self.assertIn('/published-baseline/aarch64', check.call_args_list[2].args)
        self.assertIn('--repository-url', check.call_args_list[1].args)
        self.assertFalse((candidate / 'release.json').exists())

    def test_upgrade_baseline_download_pins_the_release_and_only_required_assets(self) -> None:
        for architecture, version in (('amd64', '0.1.1'), ('aarch64', '0.1.3')):
            with self.subTest(architecture=architecture), patch.object(ci, 'run') as run:
                directory = ci.download_upgrade_baseline(architecture)
            self.assertEqual(directory, Path('.tmp/published-baseline') / architecture)
            self.assertTrue(directory.is_dir())
            arguments = run.call_args.args
            self.assertEqual(arguments[:4], ('gh', 'release', 'download', 'v' + version))
            self.assertEqual(
                arguments[9::2],
                (
                    'release.json',
                    'SHA256SUMS',
                    'checksums.sigstore.jsonl',
                    'artifacts.sigstore.jsonl',
                    f'garmin-toolkit-hass-{version}-{architecture}.oci.tar',
                ),
            )
            self.assertNotIn('--clobber', arguments)
        with patch.object(ci, 'run') as run, self.assertRaises(ValueError):
            ci.download_upgrade_baseline('unknown')
        run.assert_not_called()

    def test_missing_published_baseline_blocks_acceptance_and_signing(self) -> None:
        with (
            patch.object(ci, 'release_guard'),
            patch.object(ci, 'build', return_value=Path('/repository')),
            patch.object(ci, 'prepare_publication'),
            patch.object(ci, 'login'),
            patch.object(ci, 'stage_publication'),
            patch.object(
                ci,
                'download_upgrade_baseline',
                side_effect=[Path('/published-amd64'), subprocess.CalledProcessError(1, ['gh'])],
            ),
            patch.object(ci, 'acceptance') as check,
            patch.object(ci, 'outputs') as outputs,
            self.assertRaises(subprocess.CalledProcessError),
        ):
            ci.draft()
        check.assert_not_called()
        outputs.assert_not_called()

    def test_completion_uses_branch_verification_or_tagged_draft_publication(self) -> None:
        for reference in ('refs/heads/dev', 'refs/tags/v0.1.1'):
            with (
                self.subTest(reference=reference),
                patch.dict(os.environ, GITHUB_REF=reference),
                patch.object(ci.hass_preflight, 'verify') as verify,
                patch.object(ci.hass_preflight, 'qualify_rejections') as reject,
                patch.object(ci, 'complete_publication') as publish,
            ):
                ci.main(tyro.cli(ci.Options, args=['complete']))
            self.assertEqual(verify.called, reference.startswith('refs/heads/'))
            self.assertEqual(reject.called, verify.called)
            self.assertEqual(publish.called, not verify.called)

    def test_acceptance_requires_exactly_one_archive(self) -> None:
        directory = self.scratch / 'native'
        directory.mkdir()
        with patch.object(ci, 'run_owned') as run:
            for count in range(3):
                if count:
                    (directory / f'{count}.oci.tar').touch()
                if count == 1:
                    ci.acceptance(directory, self.scratch / 'evidence')
                    self.assertIn(str(directory / '1.oci.tar'), run.call_args.args)
                else:
                    with self.assertRaisesRegex(ValueError, 'exactly one'):
                        ci.acceptance(directory, self.scratch / 'evidence')
            self.assertEqual(run.call_count, 1)

    def test_native_acceptance_selects_guest_metadata_and_retains_bounded_lifecycle_checks(self) -> None:
        directory = self.scratch / 'native'
        directory.mkdir()
        archive = directory / 'image.oci.tar'
        archive.touch()
        evidence = self.scratch / 'evidence'
        image = self.scratch / 'pinned-haos.qcow2.xz'
        for system, architecture, attribute in (
            ('x86_64-linux', 'amd64', '.#garmin-hass-app-repository'),
            ('aarch64-linux', 'aarch64', '.#garmin-hass-arm64-test-repository'),
        ):
            with (
                self.subTest(system=system),
                patch.object(
                    ci, 'verify_build_inputs', return_value={'platform': {'architecture': architecture}}
                ) as verify,
                patch.object(ci, 'build', return_value=Path('/private-metadata')) as build,
                patch.object(ci, 'run_owned') as run,
            ):
                ci.main(
                    tyro.cli(
                        ci.Options,
                        args=[
                            'acceptance',
                            '--system',
                            system,
                            '--build-directory',
                            str(directory),
                            '--evidence-directory',
                            str(evidence),
                            '--image',
                            str(image),
                        ],
                    )
                )
            verify.assert_called_once_with(archive, directory / 'native-build.json', self.root)
            build.assert_called_once_with(attribute)
            arguments = run.call_args.args
            self.assertIn(str(archive), arguments)
            self.assertIn(str(evidence), arguments)
            self.assertEqual(arguments[arguments.index('--arch') + 1], architecture)
            self.assertEqual(arguments[arguments.index('--repository') + 1], '/private-metadata')
            self.assertEqual(arguments[arguments.index('--image') + 1], str(image))
            self.assertEqual(arguments[arguments.index('--timeout') + 1], '1800')
            self.assertIn('--lifecycle', arguments)
            self.assertNotIn('--baseline-release', arguments)
            self.assertNotIn('--repository-url', arguments)
            self.assertEqual(run.call_args.kwargs['timeout'], 1900)

    def test_native_acceptance_rejects_invalid_inputs_before_metadata_or_vm_start(self) -> None:
        directory = self.scratch / 'native'
        directory.mkdir()
        options = ci.Options(command='acceptance', system='aarch64-linux', build_directory=directory)
        with patch.object(ci, 'build') as build, patch.object(ci, 'run_owned') as run:
            for count in range(3):
                if count:
                    (directory / f'{count}.oci.tar').touch()
                if count == 1:
                    with (
                        patch.object(ci, 'verify_build_inputs', return_value={'platform': {'architecture': 'amd64'}}),
                        self.assertRaisesRegex(ValueError, 'platform differs'),
                    ):
                        ci.native_acceptance(options)
                else:
                    with self.assertRaisesRegex(ValueError, 'exactly one'):
                        ci.native_acceptance(options)
            build.assert_not_called()
            run.assert_not_called()

    def test_delivery_download_binds_a_trusted_ci_head_or_pr_merge_to_verified_native_inputs(self) -> None:
        for merge in (False, True):
            document, receipt = delivery_inputs()
            if merge:
                document['head_repository'] = {'full_name': 'contributor/fork'}
            else:
                document['event'] = 'push'
                object_value(receipt['source'])['commit'] = document['head_sha']
            directory = self.scratch / str(merge)

            def execute(
                *arguments: str,
                document: Object = document,
                receipt: Object = receipt,
                directory: Path = directory,
                **_keywords: object,
            ) -> str:
                if arguments[:2] == ('gh', 'api'):
                    return json_bytes(document).decode()
                if arguments[:3] == ('gh', 'run', 'download'):
                    directory.mkdir()
                    (directory / 'native-build.json').write_bytes(json_bytes(receipt))
                    (directory / 'image.oci.tar').touch()
                if arguments[:2] == ('git', 'show'):
                    return 'f' * 40 + ' ' + 'a' * 40
                return ''

            with (
                self.subTest(merge=merge),
                patch.object(ci, 'run', side_effect=execute) as run,
                patch.object(ci, 'verify_build_inputs', return_value=receipt) as verify,
            ):
                result = ci.download_delivery_build(
                    ci.Options(command='delivery', source_run=456, build_directory=directory)
                )
            self.assertEqual(result, receipt)
            verify.assert_called_once_with(directory / 'image.oci.tar', directory / 'native-build.json', self.root)
            self.assertIn(
                (
                    'gh',
                    'run',
                    'download',
                    '456',
                    '--repo',
                    'kubijo/garmin-toolkit',
                    '--name',
                    'garmin-hass-native-aarch64',
                    '--dir',
                    str(directory),
                ),
                [call.args for call in run.call_args_list],
            )

    def test_delivery_rejects_untrusted_failed_or_unrelated_runs_before_downloading(self) -> None:
        for field, invalid in (
            ('id', 457),
            ('path', '.github/workflows/untrusted.yml'),
            ('event', 'pull_request_target'),
            ('status', 'in_progress'),
            ('conclusion', 'failure'),
            ('head_sha', 'not-a-commit'),
            ('repository', {'full_name': 'someone/other'}),
        ):
            document, _receipt = delivery_inputs()
            document[field] = invalid
            with (
                self.subTest(field=field),
                patch.object(ci, 'run', return_value=json_bytes(document).decode()) as run,
                self.assertRaises(ValueError),
            ):
                ci.download_delivery_build(
                    ci.Options(command='delivery', source_run=456, build_directory=self.scratch / 'native')
                )
            self.assertEqual(run.call_count, 1)
        document, _receipt = delivery_inputs()
        with (
            patch.object(
                ci, 'run', side_effect=[json_bytes(document).decode(), subprocess.CalledProcessError(1, 'git')]
            ) as run,
            self.assertRaises(subprocess.CalledProcessError),
        ):
            ci.download_delivery_build(
                ci.Options(command='delivery', source_run=456, build_directory=self.scratch / 'native')
            )
        self.assertEqual(run.call_count, 2)
        for run_id in (None, 0, -1):
            with patch.object(ci, 'run') as run, self.assertRaises(ValueError):
                ci.download_delivery_build(
                    ci.Options(command='delivery', source_run=run_id, build_directory=self.scratch / 'native')
                )
            run.assert_not_called()

    def test_delivery_rejects_misattributed_receipts_before_payload_admission(self) -> None:
        for case in ('workflow', 'architecture', 'commit', 'merge'):
            document, receipt = delivery_inputs()
            platform = object_value(receipt['platform'])
            if case == 'workflow':
                platform['workflow_run'] = 'https://github.com/kubijo/garmin-toolkit/actions/runs/457'
            elif case == 'architecture':
                platform['architecture'] = 'amd64'
            elif case == 'commit':
                object_value(receipt['source'])['commit'] = 'invalid'
            directory = self.scratch / case

            def execute(
                *arguments: str,
                document: Object = document,
                receipt: Object = receipt,
                directory: Path = directory,
                **_keywords: object,
            ) -> str:
                if arguments[:2] == ('gh', 'api'):
                    return json_bytes(document).decode()
                if arguments[:3] == ('gh', 'run', 'download'):
                    directory.mkdir()
                    (directory / 'native-build.json').write_bytes(json_bytes(receipt))
                    (directory / 'image.oci.tar').touch()
                if arguments[:2] == ('git', 'show'):
                    return 'f' * 40 + ' ' + 'c' * 40
                return ''

            with (
                self.subTest(case=case),
                patch.object(ci, 'run', side_effect=execute),
                patch.object(ci, 'verify_build_inputs') as verify,
                self.assertRaises(ValueError),
            ):
                ci.download_delivery_build(ci.Options(command='delivery', source_run=456, build_directory=directory))
            verify.assert_not_called()

    def test_delivery_evidence_requires_the_staged_image_and_complete_runtime_oracles(self) -> None:
        _document, receipt = delivery_inputs()
        platform = object_value(receipt['platform'])
        staging: Object = {
            'tag': '0.1.2-candidate.123.1',
            'repository_url': publication.REMOTE + '#hass-candidate-123-1',
            'index_digest': 'sha256:' + 'f' * 64,
        }
        installed: Object = {
            **{key: platform[key] for key in ('archive_sha256', 'manifest_digest', 'config_digest')},
            'architecture': 'arm64',
            'archive_version': '0.1.2',
            'version': staging['tag'],
            'repository_url': staging['repository_url'],
            'registry_digest': staging['index_digest'],
        }
        workflow: Object = {
            **{key: True for key in ('passed', 'restart', 'cold_restore', 'post_backup_mutation_reverted')},
            'haos_reboot': {'before': 'old-boot', 'after': 'new-boot'},
        }
        evidence = self.scratch / 'evidence'
        evidence.mkdir()
        inputs = {'workflow.json': workflow, 'installation.json': installed, 'native-build.json': receipt}
        for name, contents in inputs.items():
            (evidence / name).write_bytes(json_bytes(contents))
        ci.verify_delivery(evidence, receipt, staging)
        mutations: list[tuple[str, str, object]] = [
            *[
                ('workflow.json', key, False)
                for key in ('passed', 'restart', 'cold_restore', 'post_backup_mutation_reverted')
            ],
            ('workflow.json', 'haos_reboot', {'before': 'same', 'after': 'same'}),
            *[('installation.json', key, 'wrong') for key in installed],
            ('native-build.json', 'source', {'commit': 'f' * 40}),
        ]
        for name, key, value in mutations:
            changed = deepcopy(inputs[name])
            changed[key] = value
            (evidence / name).write_bytes(json_bytes(changed))
            with self.subTest(name=name, key=key), self.assertRaises(ValueError):
                ci.verify_delivery(evidence, receipt, staging)
            (evidence / name).write_bytes(json_bytes(inputs[name]))

    def test_delivery_verifier_accepts_actual_remote_installation_reports_and_rejects_wrong_supervisor_versions(
        self,
    ) -> None:
        archive = self.scratch / 'image.oci.tar'
        write_image(archive, architecture='arm64', version='0.1.2')
        inspected = vm.inspect_archive(archive)
        _document, receipt = delivery_inputs()
        object_value(receipt['platform']).update(
            archive_sha256=file_sha256(archive),
            manifest_digest=inspected.manifest_digest,
            config_digest=inspected.config_digest,
        )
        for case in ('candidate-tag', 'release-tag', 'wrong-supervisor-version'):
            tag = '0.1.2' if case == 'release-tag' else '0.1.2-candidate.123.1'
            staging: Object = {
                'tag': tag,
                'repository_url': publication.REMOTE + '#hass-candidate-123-1',
                'index_digest': 'sha256:' + 'f' * 64,
            }
            reference = publication.IMAGE + ':' + tag
            evidence = self.scratch / case
            evidence.mkdir()

            def guest(
                command: list[str],
                *,
                reference: str = reference,
                timeout: int = 30,
            ) -> subprocess.CompletedProcess[str]:
                if command[-1].startswith('docker inspect '):
                    response: Object = {'Image': inspected.manifest_digest, 'Config': {'Image': reference}}
                elif command[-1].startswith('docker image inspect '):
                    response = {
                        'Id': inspected.manifest_digest,
                        'Architecture': 'arm64',
                        'Os': 'linux',
                        'RepoDigests': [publication.IMAGE + '@sha256:' + 'f' * 64],
                    }
                else:
                    response = {}
                return subprocess.CompletedProcess(command, 0, stdout=json.dumps(response))

            with (
                self.subTest(case=case),
                ExitStack() as services,
                patch.object(vm, 'test_config', return_value=('', '0.1.2')),
                patch.object(vm, 'registry_manifest', return_value=inspected.manifest_digest),
                patch.object(vm, 'reload_store'),
                patch.object(vm, 'run', side_effect=guest),
                patch.object(
                    vm, 'guest_data', return_value={'version': 'wrong' if case == 'wrong-supervisor-version' else tag}
                ),
            ):
                options = vm.Options(
                    app_archive=archive,
                    arch='aarch64',
                    repository_url=str(staging['repository_url']),
                    registry_digest=str(staging['index_digest']),
                    registry_tag=None if case == 'release-tag' else tag,
                )
                if case == 'wrong-supervisor-version':
                    with self.assertRaisesRegex(RuntimeError, 'expected app version'):
                        vm.install_app(options, evidence, self.root / 'key', 2222, services)
                    self.assertFalse((evidence / 'installation.json').exists())
                    continue
                vm.install_app(options, evidence, self.root / 'key', 2222, services)
            (evidence / 'native-build.json').write_bytes(json_bytes(receipt))
            (evidence / 'workflow.json').write_bytes(
                json_bytes(
                    {
                        'passed': True,
                        'restart': True,
                        'cold_restore': True,
                        'post_backup_mutation_reverted': True,
                        'haos_reboot': {'before': 'old', 'after': 'new'},
                    }
                )
            )
            ci.verify_delivery(evidence, receipt, staging)
            bound = bind_acceptance(
                (evidence,),
                {'source': receipt['source'], 'version': receipt['version'], 'platforms': [receipt['platform']]},
            )
            self.assertEqual(bound[0]['scope'], 'native-image-content')

    def test_delivery_uses_only_the_retained_arm64_image_and_remote_acceptance(self) -> None:
        _document, receipt = delivery_inputs()
        staging: Object = {
            'tag': '0.1.2-candidate.123.1',
            'repository_url': publication.REMOTE + '#hass-candidate-123-1',
            'index_digest': 'sha256:' + 'f' * 64,
        }
        options = ci.Options(
            command='delivery',
            system='aarch64-linux',
            source_run=456,
            build_directory=self.scratch / 'native',
            evidence_directory=self.scratch / 'evidence',
        )
        options.evidence_directory.mkdir()
        for name in ('workflow.json', 'installation.json'):
            (options.evidence_directory / name).write_bytes(b'{}\n')
        publication_root = self.scratch / 'publication'

        def prepare(_options: object) -> Object:
            publication_root.mkdir()
            (publication_root / 'staging.json').write_bytes(json_bytes(staging))
            return {'image': {'index_digest': staging['index_digest']}}

        with (
            patch.dict(
                os.environ,
                GITHUB_EVENT_NAME='workflow_dispatch',
                GITHUB_SHA='a' * 40,
                PUBLISH='false',
                RECOVERY='false',
                GITHUB_REPOSITORY='someone/fork',
            ),
            patch.object(ci, 'PUBLICATION', publication_root),
            patch.object(ci, 'build_source', return_value=('0.1.2', {'commit': 'a' * 40})),
            patch.object(ci, 'download_delivery_build', return_value=receipt),
            patch.object(ci, 'run', return_value='unchanged public refs'),
            patch.object(ci, 'prepare_registry', side_effect=prepare),
            patch.object(ci, 'copy_repository'),
            patch.object(ci, 'build', return_value=self.root / 'metadata') as build,
            patch.object(ci, 'login') as login,
            patch.object(ci, 'stage_image') as stage,
            patch.object(ci, 'acceptance') as accept,
            patch.object(ci, 'verify_delivery') as verify,
            patch.object(ci, 'native') as native,
            patch.object(ci, 'promote') as promote,
        ):
            ci.main(options)
        build.assert_called_once_with('.#garmin-hass-arm64-test-repository')
        login.assert_called_once()
        stage.assert_called_once()
        arguments = accept.call_args.args
        self.assertEqual(arguments[:2], (options.build_directory, options.evidence_directory))
        self.assertEqual(arguments[arguments.index('--arch') + 1], 'aarch64')
        self.assertEqual(arguments[arguments.index('--repository-url') + 1], staging['repository_url'])
        self.assertEqual(arguments[arguments.index('--registry-digest') + 1], staging['index_digest'])
        self.assertEqual(arguments[arguments.index('--registry-tag') + 1], staging['tag'])
        self.assertNotIn('--lifecycle', arguments)
        self.assertEqual(accept.call_args.kwargs['timeout'], 1900)
        verify.assert_called_once_with(options.evidence_directory, receipt, staging)
        native.assert_not_called()
        promote.assert_not_called()
        self.assertTrue(object_value(read_json(publication_root / 'delivery.json'))['published_refs_unchanged'])

    def test_delivery_rejects_wrong_modes_or_checkout_identity_before_downloading(self) -> None:
        for overrides in (
            {'PUBLISH': 'true'},
            {'RECOVERY': 'true'},
            {'GITHUB_SHA': 'b' * 40},
            {'GITHUB_EVENT_NAME': 'push'},
        ):
            with (
                self.subTest(overrides=overrides),
                patch.dict(
                    os.environ,
                    {
                        'PUBLISH': 'false',
                        'RECOVERY': 'false',
                        'GITHUB_SHA': 'a' * 40,
                        'GITHUB_EVENT_NAME': 'workflow_dispatch',
                        **overrides,
                    },
                ),
                patch.object(ci, 'build_source', return_value=('0.1.2', {'commit': 'a' * 40})),
                patch.object(ci, 'download_delivery_build') as download,
                patch.object(ci, 'login') as login,
                self.assertRaises(ValueError),
            ):
                ci.delivery(ci.Options(command='delivery', system='aarch64-linux', source_run=456))
            download.assert_not_called()
            login.assert_not_called()

    def test_promotion_and_final_install_failures_both_attempt_safe_rollback(self) -> None:
        def promoted(_options: object) -> None:
            ci.PUBLICATION.mkdir()
            (ci.PUBLICATION / 'staging.json').write_bytes(json_bytes({'index_digest': 'sha256:' + 'a' * 64}))

        with (
            patch.object(ci, 'release_guard'),
            patch.object(ci, 'login'),
            patch.object(ci, 'promote_draft', side_effect=promoted) as promote,
            patch.object(ci, 'acceptance', side_effect=RuntimeError('guest failed')),
            patch.object(ci, 'rollback_store') as rollback,
        ):
            with self.assertRaisesRegex(RuntimeError, 'guest failed'):
                ci.promote()
            rollback.assert_called_once()
            rollback.reset_mock()
            shutil.rmtree(ci.PUBLICATION)
            promote.side_effect = ValueError('untrusted draft')
            with self.assertRaisesRegex(ValueError, 'untrusted draft'):
                ci.promote()
            rollback.assert_called_once()

    def test_closed_output_cannot_prevent_promotion_rollback(self) -> None:
        failure = RuntimeError('promotion failed')
        with (
            patch.object(ci, 'release_guard'),
            patch.object(ci, 'login'),
            patch.object(ci, 'promote_draft', side_effect=failure),
            patch.object(ci, 'rollback_store') as rollback,
            patch('builtins.print', side_effect=BrokenPipeError('pipe closed')),
            self.assertRaises(RuntimeError) as caught,
        ):
            ci.promote()
        self.assertIs(caught.exception, failure)
        rollback.assert_called_once()

    def test_reused_scratch_cannot_roll_back_a_previous_successful_promotion(self) -> None:
        ci.PUBLICATION.mkdir()
        (ci.PUBLICATION / 'promotion.json').write_bytes(
            json_bytes({'previous': 'a' * 40, 'published': 'b' * 40, 'qualified': True})
        )
        with (
            patch.object(ci, 'release_guard'),
            patch.object(ci, 'login') as login,
            patch.object(ci, 'rollback_store') as rollback,
            self.assertRaisesRegex(ValueError, 'already exists'),
        ):
            ci.promote()
        login.assert_not_called()
        rollback.assert_not_called()

    def run_promotion(self, root: Path, mode: str, previous: str) -> int:
        process = get_context('spawn').Process(target=promotion_worker, args=(root, mode, previous))
        process.start()
        try:
            process.join(20)
            self.assertFalse(process.is_alive(), 'Promotion fixture did not exit')
            if process.exitcode is None:
                raise AssertionError('Promotion fixture has no exit status')
            return process.exitcode
        finally:
            if process.is_alive():
                process.kill()
                process.join(5)
            process.close()

    def prepare_store(self, name: str, *, existing: bool) -> tuple[Path, str]:
        root = self.root / name
        root.mkdir()
        subprocess.run(['git', 'init', '--bare', '-q', str(root / 'remote.git')], check=True)
        metadata = root / 'metadata'
        metadata.mkdir()
        config = metadata / 'repository.yaml'
        config.write_text('previous synthetic metadata\n')
        with patch.object(publication, 'REMOTE', str(root / 'remote.git')):
            previous = publication.push_metadata(metadata, 'hass-apps', '') if existing else ''
        config.write_text('new synthetic metadata\n')
        return root, previous

    def test_signals_during_push_or_acceptance_recover_only_unfinished_changes(self) -> None:
        for existing, mode in product((False, True), ('term-push', 'kill-push', 'term-accept', 'kill-accept')):
            with self.subTest(existing=existing, mode=mode):
                root, previous = self.prepare_store(f'{existing}-{mode}', existing=existing)
                self.assertEqual(self.run_promotion(root, mode, previous), -9 if mode.startswith('kill-') else 143)
                with patch.object(publication, 'REMOTE', str(root / 'remote.git')):
                    head = publication.branch_head('hass-apps')
                    self.assertEqual(head == previous, mode.startswith('term-'))
                    directory = root / ('publication-' + mode)
                    for _ in range(2):
                        publication.cleanup_staging(publication.CleanupStaging(directory=directory))
                        self.assertEqual(publication.branch_head('hass-apps'), previous)

    def test_success_and_failed_retries_preserve_the_inherited_store(self) -> None:
        for existing in (False, True):
            with self.subTest(existing=existing):
                root, previous = self.prepare_store(f'retry-{existing}', existing=existing)
                self.assertEqual(self.run_promotion(root, 'success', previous), 0)
                with patch.object(publication, 'REMOTE', str(root / 'remote.git')):
                    published = publication.branch_head('hass-apps')
                    publication.cleanup_staging(publication.CleanupStaging(directory=root / 'publication-success'))
                    self.assertEqual(publication.branch_head('hass-apps'), published)
                    for mode in ('retry-copy-fail', 'retry-accept-fail'):
                        self.assertEqual(self.run_promotion(root, mode, previous), 1)
                        directory = root / ('publication-' + mode)
                        journal = object_value(read_json(directory / 'promotion.json'))
                        self.assertEqual(journal['previous'], published)
                        publication.cleanup_staging(publication.CleanupStaging(directory=directory))
                        self.assertEqual(publication.branch_head('hass-apps'), published)

    def test_failed_retry_recovers_an_abandoned_unqualified_publication(self) -> None:
        for existing, mode in product((False, True), ('retry-copy-fail', 'retry-accept-fail')):
            with self.subTest(existing=existing, mode=mode):
                root, previous = self.prepare_store(f'abandoned-{existing}-{mode}', existing=existing)
                self.assertEqual(self.run_promotion(root, 'kill-accept', previous), -9)
                with patch.object(publication, 'REMOTE', str(root / 'remote.git')):
                    self.assertNotEqual(publication.branch_head('hass-apps'), previous)
                    self.assertEqual(self.run_promotion(root, mode, previous), 1)
                    directory = root / ('publication-' + mode)
                    publication.cleanup_staging(publication.CleanupStaging(directory=directory))
                    self.assertEqual(publication.branch_head('hass-apps'), previous)

    def test_remote_qualification_survives_a_failed_local_completion_write(self) -> None:
        root, previous = self.prepare_store('completion', existing=True)
        self.assertEqual(self.run_promotion(root, 'qualify-journal-fail', previous), 1)
        directory = root / 'publication-qualify-journal-fail'
        self.assertIs(object_value(read_json(directory / 'promotion.json'))['qualified'], False)
        with patch.object(publication, 'REMOTE', str(root / 'remote.git')):
            published = publication.branch_head('hass-apps')
            self.assertNotEqual(published, previous)
            publication.cleanup_staging(publication.CleanupStaging(directory=directory))
            self.assertEqual(publication.branch_head('hass-apps'), published)
            self.assertEqual(self.run_promotion(root, 'success', previous), 0)

    def wait_file(self, path: Path) -> None:
        deadline = time.monotonic() + 10
        while not path.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue(path.exists(), 'Fixture checkpoint did not arrive: ' + path.name)

    def test_recovery_fences_a_push_still_running_at_the_remote(self) -> None:
        for existing in (False, True):
            with self.subTest(existing=existing):
                self.check_inflight_push(existing=existing)

    def check_inflight_push(self, *, existing: bool) -> None:
        root, previous = self.prepare_store(f'inflight-{existing}', existing=existing)
        hook = root / 'remote.git/hooks/pre-receive'
        hook.write_text(
            '#!' + sys.executable + '\n'
            'import pathlib, sys, time\n'
            f'root = pathlib.Path({str(root)!r})\n'
            'if any(row.rstrip().endswith("refs/heads/hass-apps") for row in sys.stdin):\n'
            '    (root / "ready").touch()\n'
            '    deadline = time.monotonic() + 15\n'
            '    while not (root / "release").exists() and time.monotonic() < deadline: time.sleep(0.02)\n'
            '    if not (root / "release").exists(): raise SystemExit(1)\n'
        )
        hook.chmod(0o700)
        receiver = root / 'receive-pack'
        receiver.write_text(
            '#!' + sys.executable + '\n'
            'import pathlib, subprocess, sys\n'
            'result = subprocess.run(["git-receive-pack", *sys.argv[1:]], check=False)\n'
            f'pathlib.Path({str(root / "done")!r}).touch()\n'
            'raise SystemExit(result.returncode)\n'
        )
        receiver.chmod(0o700)
        process = get_context('spawn').Process(target=promotion_worker, args=(root, 'inflight', previous))
        process.start()
        try:
            self.wait_file(root / 'ready')
            os.kill(process.pid or 0, signal.SIGTERM)
            process.join(10)
            self.assertEqual(process.exitcode, 143)
            with patch.object(publication, 'REMOTE', str(root / 'remote.git')):
                publication.cleanup_staging(publication.CleanupStaging(directory=root / 'publication-inflight'))
                (root / 'release').touch()
                self.wait_file(root / 'done')
                self.assertEqual(publication.branch_head('hass-apps'), previous)
        finally:
            (root / 'release').touch()
            if process.is_alive():
                process.kill()
                process.join(5)
            process.close()
            self.wait_file(root / 'done')

    @unittest.skipUnless(USER_MANAGER, 'requires a systemd user manager')
    def test_termination_allows_acceptance_to_clean_its_detached_child(self) -> None:
        for direct in (False, True):
            with self.subTest(direct=direct):
                self.check_child_shutdown(direct=direct)

    def check_child_shutdown(self, *, direct: bool) -> None:
        root, previous = self.prepare_store(f'child-{direct}', existing=False)
        (root / 'acceptance.py').write_text(
            'import os, pathlib, signal, subprocess, sys\n'
            f'root = pathlib.Path({str(root)!r})\n'
            'def stopped(*args): raise SystemExit(143)\n'
            'signal.signal(signal.SIGTERM, stopped)\n'
            'child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)\n'
            '(root / "ready").write_text(str(child.pid))\n'
            'try:\n'
            '    child.wait()\n'
            'finally:\n'
            '    os.killpg(child.pid, signal.SIGTERM)\n'
            '    child.wait(timeout=5)\n'
            '    (root / "cleaned").touch()\n'
        )
        process = (
            get_context('spawn').Process(target=ci.run_owned, args=(sys.executable, str(root / 'acceptance.py')))
            if direct
            else get_context('spawn').Process(target=promotion_worker, args=(root, 'child-accept', previous))
        )
        process.start()
        try:
            self.wait_file(root / 'ready')
            os.kill(process.pid or 0, signal.SIGTERM)
            process.join(10)
            self.assertEqual(process.exitcode, 143)
            self.assertTrue((root / 'cleaned').exists())
            with self.assertRaises(ProcessLookupError):
                os.kill(int((root / 'ready').read_text()), 0)
        finally:
            if process.is_alive():
                process.kill()
                process.join(5)
            process.close()
            if (root / 'ready').exists():
                with suppress(ProcessLookupError):
                    os.killpg(int((root / 'ready').read_text()), signal.SIGKILL)

    @unittest.skipUnless(USER_MANAGER, 'requires a systemd user manager')
    def test_owned_command_timeout_escalates_after_its_grace_period(self) -> None:
        marker = self.root / 'uncooperative-pid'
        command = (
            'import os, pathlib, signal, time; '
            'signal.signal(signal.SIGTERM, signal.SIG_IGN); '
            f'pathlib.Path({str(marker)!r}).write_text(str(os.getpid())); '
            'time.sleep(60)'
        )
        with self.assertRaises(subprocess.TimeoutExpired):
            ci.run_owned(sys.executable, '-c', command, timeout=1, grace=0.05)
        with self.assertRaises(ProcessLookupError):
            os.kill(int(marker.read_text()), 0)

    def test_output_rejects_line_injection_without_partial_write(self) -> None:
        with self.assertRaisesRegex(ValueError, 'single-line'):
            ci.outputs(valid='good', malicious='bad\nindex=injected')
        self.assertFalse((self.root / 'outputs').exists())

    def test_host_setup_refuses_local_machine(self) -> None:
        for command in ('prepare', 'limits'):
            result = subprocess.run(
                ['bash', str(RUNNER), command],
                env={**os.environ, 'RUNNER_ENVIRONMENT': 'self-hosted'},
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('disposable GitHub-hosted', result.stderr)

    def test_host_limits_must_be_confirmed_by_systemd(self) -> None:
        for name, script in {
            'sudo': '#!/bin/sh\nexit 0\n',
            'systemctl': '#!/bin/sh\ncase "$4" in\n'
            'MemoryMax) printf %s "$TEST_MEMORY";;\n'
            'CPUQuotaPerSecUSec) printf %s "$TEST_CPU";;\nesac\n',
        }.items():
            binary = self.root / name
            binary.write_text(script)
            binary.chmod(0o700)
        for memory, cpu, success in (
            ('6442450944', '2s', True),
            ('infinity', '2s', False),
            ('6442450944', '3s', False),
        ):
            result = subprocess.run(
                ['bash', str(RUNNER), 'limits'],
                check=False,
                env={
                    **os.environ,
                    'PATH': str(self.root) + ':' + os.environ['PATH'],
                    'GITHUB_ACTIONS': 'true',
                    'RUNNER_ENVIRONMENT': 'github-hosted',
                    'RUNNER_OS': 'Linux',
                    'TEST_MEMORY': memory,
                    'TEST_CPU': cpu,
                },
            )
            self.assertEqual(result.returncode == 0, success)

    def test_emulated_haos_setup_starts_user_manager_and_installs_selected_firmware(self) -> None:
        log = self.root / 'setup.log'
        for name, script in {
            'sudo': '#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_SETUP_LOG"\n',
            'systemctl': '#!/bin/sh\nexit 0\n',
        }.items():
            binary = self.root / name
            binary.write_text(script)
            binary.chmod(0o700)
        for selected, expected in (
            ('true', 'qemu-system-x86 qemu-utils ovmf'),
            ('x86_64-linux', 'qemu-system-x86 qemu-utils ovmf'),
            (
                'aarch64-linux',
                'qemu-system-arm qemu-efi-aarch64 qemu-utils ipxe-qemu ipxe-qemu-256k-compat-efi-roms seabios',
            ),
            (
                'all',
                'qemu-system-x86 qemu-system-arm qemu-efi-aarch64 qemu-utils ovmf '
                'ipxe-qemu ipxe-qemu-256k-compat-efi-roms seabios',
            ),
        ):
            with self.subTest(selected=selected):
                log.unlink(missing_ok=True)
                result = subprocess.run(
                    ['bash', str(RUNNER), 'prepare', 'false', selected],
                    check=False,
                    capture_output=True,
                    text=True,
                    env={
                        **os.environ,
                        'PATH': str(self.root) + ':' + os.environ['PATH'],
                        'GITHUB_ENV': str(self.root / 'github-env'),
                        'TEST_SETUP_LOG': str(log),
                    },
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                actions = log.read_text()
                self.assertIn('loginctl enable-linger', actions)
                self.assertIn('systemctl start user@', actions)
                self.assertIn(expected, actions)
                self.assertNotIn('/dev/kvm', actions)
        log.unlink()
        result = subprocess.run(
            ['bash', str(RUNNER), 'prepare', 'false', 'unsupported-guest'],
            check=False,
            env={**os.environ, 'PATH': str(self.root) + ':' + os.environ['PATH'], 'TEST_SETUP_LOG': str(log)},
        )
        self.assertEqual(result.returncode, 64)
        self.assertFalse(log.exists())

    def test_aggregation_requires_every_job_success(self) -> None:
        for results, success in (
            (['success', 'success'], True),
            (['success', 'failure'], False),
            (['success', 'skipped'], False),
            (['cancelled'], False),
            ([], False),
        ):
            result = subprocess.run(['bash', str(RUNNER), 'require-success', *results], check=False)
            self.assertEqual(result.returncode == 0, success)

    def test_shell_cleanup_attempts_recovery_and_reports_either_failure(self) -> None:
        binary = self.root / 'nix'
        binary.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$TEST_RECOVERY_LOG"\nexit "$TEST_RECOVERY_STATUS"\n')
        binary.chmod(0o700)
        for removal_fails, recovery_status in product((False, True), (0, 42)):
            with self.subTest(removal_fails=removal_fails, recovery_status=recovery_status):
                case = self.root / f'{removal_fails}-{recovery_status}'
                directory = case / '.tmp/docker-auth'
                directory.mkdir(parents=True)
                credentials = directory / 'config.json'
                if removal_fails:
                    credentials.mkdir()
                else:
                    credentials.write_text('synthetic credentials')
                log = self.root / f'recovery-{removal_fails}-{recovery_status}'
                result = subprocess.run(
                    ['bash', str(RUNNER), 'cleanup'],
                    env={
                        **os.environ,
                        'PATH': str(self.root) + ':' + os.environ['PATH'],
                        'TEST_RECOVERY_LOG': str(log),
                        'TEST_RECOVERY_STATUS': str(recovery_status),
                    },
                    cwd=case,
                    check=False,
                    capture_output=True,
                    timeout=5,
                )
                self.assertEqual(result.returncode, recovery_status or int(removal_fails))
                self.assertEqual(log.read_text(), 'run\n.#hass-ci\n--\ncleanup\n')
                self.assertFalse(credentials.is_file())


if __name__ == '__main__':
    unittest.main()
