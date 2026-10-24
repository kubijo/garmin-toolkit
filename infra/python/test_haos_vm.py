"""Exercise image admission, native selection, and owned-session cleanup."""

import base64
import gzip
import hashlib
import io
import json
import os
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
import urllib.request
from collections.abc import Callable
from copy import deepcopy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import Mock, call, patch

import haos_app_session as session
import haos_app_vm as vm
from fixtures.images import tar_bytes, write_image
from haos_platform import (
    BASELINE_VERSION,
    PLATFORMS,
    acceleration,
    baseline_fixture,
    inspect_archive,
    resolve,
    unpack_archive,
    validate_archive,
)
from json_data import object_value
from processes import ACCEPTANCE_SCOPE, vm_scope


class HaosTests(unittest.TestCase):
    def setUp(self) -> None:
        vm.SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix='haos-tests.', dir=vm.SCRATCH_ROOT)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def image(self, architecture: str = 'amd64', version: str = '0.1.0') -> Path:
        archive = self.root / 'app.tar'
        write_image(archive, architecture=architecture, version=version)
        return archive

    def test_installation_link_acceptance_requires_the_public_repository(self) -> None:
        for repository in (None, 'https://github.com/kubijo/garmin-toolkit#different'):
            with self.subTest(repository=repository), self.assertRaisesRegex(ValueError, 'public hass-apps'):
                session.run_session(
                    session.Check(app_archive=self.root / 'unused', repository_url=repository, install_links=True)
                )

    def test_auto_matches_native_architecture_and_rejects_unknown_host(self) -> None:
        for machine, expected in [('x86_64', 'amd64'), ('aarch64', 'aarch64'), ('arm64', 'aarch64')]:
            with self.subTest(machine=machine), patch('haos_platform.platform.machine', return_value=machine):
                self.assertEqual(resolve().architecture, expected)
        with patch('haos_platform.platform.machine', return_value='unknown'), self.assertRaises(ValueError):
            resolve()

    def test_baseline_fixture_preserves_candidate_bytes_and_filesystem_layers(self) -> None:
        source = self.image()
        before = source.read_bytes()
        fixture = self.root / 'baseline.oci.tar'
        baseline_fixture(source, fixture)
        original, previous = inspect_archive(source), inspect_archive(fixture)
        validate_archive(fixture, PLATFORMS['amd64'], BASELINE_VERSION, inspected=previous)
        self.assertEqual(previous.layers, original.layers)
        self.assertEqual(previous.configuration['rootfs'], original.configuration['rootfs'])
        with tarfile.open(source) as source_tar, tarfile.open(fixture) as fixture_tar:
            for name in original.layers:
                source_layer, fixture_layer = source_tar.extractfile(name), fixture_tar.extractfile(name)
                assert source_layer is not None and fixture_layer is not None
                with source_layer, fixture_layer:
                    self.assertEqual(source_layer.read(), fixture_layer.read())
        self.assertNotEqual(previous.config_digest, original.config_digest)
        self.assertEqual(previous.repository_tags, ('ghcr.io/kubijo/garmin-toolkit-hass:' + BASELINE_VERSION,))
        self.assertEqual(source.read_bytes(), before)
        with self.assertRaises(FileExistsError):
            baseline_fixture(source, fixture)

    def test_kvm_requires_native_guest_and_device_access(self) -> None:
        with patch('haos_platform.platform.machine', return_value='x86_64'):
            with patch('haos_platform.os.access', return_value=True):
                self.assertEqual(acceleration(PLATFORMS['amd64']), 'kvm')
                self.assertEqual(acceleration(PLATFORMS['aarch64']), 'tcg,thread=multi')
            with patch('haos_platform.os.access', return_value=False):
                self.assertEqual(acceleration(PLATFORMS['amd64']), 'tcg,thread=multi')

    def test_guest_ssh_uses_only_its_disposable_identity(self) -> None:
        command = vm.ssh_command(self.root / 'key', 2222, 'true')
        self.assertEqual(command[command.index('-F') + 1], '/dev/null')
        self.assertIn('IdentitiesOnly=yes', command)
        self.assertIn('IdentityAgent=none', command)

    def test_native_publish_preserves_digests_and_closes_tunnel_after_failure(self) -> None:
        archive = self.image()
        tunnel = Mock(spec=subprocess.Popen)
        tunnel.poll.return_value = None
        with (
            patch.object(vm, 'forward_port', return_value=tunnel),
            patch.object(vm.socket, 'socket') as connection,
            patch.object(vm, 'run', side_effect=RuntimeError('push failed')) as push,
            patch.object(vm, 'stop_group') as stop,
        ):
            connection.return_value.__enter__.return_value.connect_ex.return_value = 0
            with self.assertRaisesRegex(RuntimeError, 'push failed'):
                vm.publish_image(archive, '0.1.0', self.root / 'key', 2222, self.root)
        command = push.call_args.args[0]
        self.assertEqual(command[0], 'skopeo')
        self.assertIn('--preserve-digests', command)
        self.assertFalse(any(self.root.glob('image-import.*')))
        stop.assert_called_once_with(tunnel)

    def test_private_supervisor_version_keeps_the_native_archive_unchanged(self) -> None:
        archive = self.image()
        before = archive.read_bytes()
        tunnel = Mock(spec=subprocess.Popen)
        tunnel.poll.return_value = None
        with (
            patch.object(vm, 'forward_port', return_value=tunnel),
            patch.object(vm.socket, 'socket') as connection,
            patch.object(vm, 'run', side_effect=RuntimeError('stop after copy')) as copy,
            patch.object(vm, 'stop_group'),
        ):
            connection.return_value.__enter__.return_value.connect_ex.return_value = 0
            with self.assertRaisesRegex(RuntimeError, 'stop after copy'):
                vm.publish_image(archive, '0.1.0', self.root / 'key', 2222, self.root, registry_tag='0.1.1-dev')
        self.assertTrue(copy.call_args.args[0][-1].endswith('/garmin-toolkit-hass:0.1.1-dev'))
        self.assertIn('--preserve-digests', copy.call_args.args[0])
        self.assertEqual(archive.read_bytes(), before)

    def test_http_redirect_cannot_escape_guest_origin(self) -> None:
        followed: list[str] = []

        class Redirect(BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                if self.path == '/unrelated':
                    followed.append(self.path)
                self.send_response(307)
                self.send_header('Location', '/unrelated')
                self.end_headers()

        with ThreadingHTTPServer(('127.0.0.1', 0), Redirect) as server:
            worker = threading.Thread(target=server.serve_forever)
            worker.start()
            try:
                with self.assertRaisesRegex(RuntimeError, 'HTTP 307'):
                    session.request_json(f'http://127.0.0.1:{server.server_port}', '/api/onboarding')
                self.assertEqual(followed, [])
            finally:
                server.shutdown()
                worker.join(timeout=3)

    def test_http_tunnel_uses_the_guest_reported_core_port(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        with (
            patch.object(vm, 'guest_data', return_value={'port': 80, 'ssl': False}),
            patch('haos_app_session.subprocess.Popen') as start,
        ):
            session.launch_tunnel(process, self.root / 'key', 2222, 18123, self.root, float('inf'))
        command = start.call_args.args[0]
        self.assertIn('127.0.0.1:18123:127.0.0.1:80', command)
        self.assertIn('ExitOnForwardFailure=yes', command)
        self.assertIn('IdentityAgent=none', command)
        self.assertTrue(start.call_args.kwargs['start_new_session'])

    def test_http_tunnel_waits_for_core_to_replace_landing_page(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        with (
            patch.object(
                vm,
                'guest_data',
                side_effect=[
                    {'version': 'landingpage', 'port': 8123, 'ssl': False},
                    {'version': '2026.10.1', 'port': 80, 'ssl': False},
                ],
            ),
            patch('haos_app_session.time.sleep'),
            patch('haos_app_session.subprocess.Popen') as start,
        ):
            session.launch_tunnel(process, self.root / 'key', 2222, 18123, self.root, float('inf'))
        self.assertIn('127.0.0.1:18123:127.0.0.1:80', start.call_args.args[0])

    def test_http_tunnel_recovers_when_supervisor_restarts_during_core_installation(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        with (
            patch.object(
                vm,
                'guest_data',
                side_effect=[
                    {'version': 'landingpage', 'port': 8123, 'ssl': False},
                    RuntimeError('ssh failed (1): supervisor/core/info: connect: connection refused'),
                    subprocess.TimeoutExpired('ha core info', 30),
                    {'version': '2026.10.2', 'port': 80, 'ssl': False},
                ],
            ) as probe,
            patch.object(session.time, 'sleep'),
            patch.object(vm, 'forward_port') as forward,
        ):
            tunnel = session.launch_tunnel(process, self.root / 'key', 2222, 18123, self.root, float('inf'))
        self.assertEqual(probe.call_count, 4)
        forward.assert_called_once_with(self.root / 'key', 2222, 18123, 80, self.root / 'tunnel.log')
        self.assertIs(tunnel, forward.return_value)

    def test_http_tunnel_stops_retrying_at_the_existing_deadline(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        failure = RuntimeError('ssh failed (1): supervisor/core/info: connect: connection refused')
        with (
            patch.object(vm, 'guest_data', side_effect=failure) as probe,
            patch.object(session.time, 'monotonic', side_effect=[0.0, 0.0, 5.0, 10.0, 10.0]),
            patch.object(session.time, 'sleep'),
            patch.object(vm, 'forward_port') as forward,
            self.assertRaisesRegex(TimeoutError, 'startup window') as stopped,
        ):
            session.launch_tunnel(process, self.root / 'key', 2222, 18123, self.root, 10.0)
        self.assertIs(stopped.exception.__cause__, failure)
        self.assertGreater(probe.call_count, 0)
        self.assertLessEqual(probe.call_count, 2)
        forward.assert_not_called()

    def test_http_tunnel_stops_immediately_when_the_owned_vm_exits(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.side_effect = [None, 1]
        with (
            patch.object(vm, 'guest_data', side_effect=RuntimeError('Supervisor unavailable')) as probe,
            patch.object(session.time, 'sleep'),
            patch.object(vm, 'forward_port') as forward,
            self.assertRaisesRegex(RuntimeError, 'HAOS exited'),
        ):
            session.launch_tunnel(process, self.root / 'key', 2222, 18123, self.root, float('inf'))
        probe.assert_called_once()
        forward.assert_not_called()

    def test_http_tunnel_rejects_invalid_core_settings_without_retrying(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        for settings in ({'port': 80, 'ssl': True}, {'port': 0, 'ssl': False}, {'port': 65536, 'ssl': False}):
            with (
                self.subTest(settings=settings),
                patch.object(vm, 'guest_data', return_value={'version': '2026.10.2', **settings}) as probe,
                patch.object(session.time, 'sleep') as sleep,
                patch.object(vm, 'forward_port') as forward,
                self.assertRaisesRegex(RuntimeError, 'valid plain HTTP port'),
            ):
                session.launch_tunnel(process, self.root / 'key', 2222, 18123, self.root, float('inf'))
            probe.assert_called_once()
            sleep.assert_not_called()
            forward.assert_not_called()

    def test_core_readiness_uses_the_available_api_before_and_after_onboarding(self) -> None:
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        for onboarding, response, path in (
            (True, [{'step': 'user', 'done': False}], '/api/onboarding'),
            (False, {'providers': [{'type': 'homeassistant'}]}, '/auth/providers'),
        ):
            with (
                self.subTest(onboarding=onboarding),
                patch.object(
                    session,
                    'request_json',
                    side_effect=[response, AssertionError('Successful readiness must not retry')],
                ) as request,
                patch.object(session.time, 'sleep'),
            ):
                session.wait_for_core(process, process, 'http://127.0.0.1:18123', float('inf'), onboarding=onboarding)
            request.assert_called_once_with('http://127.0.0.1:18123', path)

    def test_onboarding_allows_slow_user_creation_without_repeating_mutations(self) -> None:
        paths: list[str] = []
        budgets: list[float] = []

        def respond(request: urllib.request.Request, *, timeout: float) -> io.BytesIO:
            path = request.selector
            paths.append(path)
            budgets.append(timeout)
            if path == '/api/onboarding/users':
                if timeout < 45:
                    raise TimeoutError('User creation needs 45 seconds')
                response: dict[str, object] = {'auth_code': 'disposable-code'}
            elif path == '/auth/token':
                response = {'access_token': 'disposable-token', 'expires_in': 1800}
            else:
                response = {}
            return io.BytesIO(json.dumps(response).encode())

        with (
            patch.object(session.HTTP, 'open', side_effect=respond),
            patch.object(session.time, 'monotonic', side_effect=[0.0, 45.0, 46.0, 47.0, 48.0]),
        ):
            tokens = session.onboard('http://127.0.0.1:18123', 'admin', deadline=120.0)
        self.assertEqual(tokens['access_token'], 'disposable-token')
        self.assertEqual(
            paths,
            [
                '/api/onboarding/users',
                '/auth/token',
                '/api/onboarding/core_config',
                '/api/onboarding/analytics',
                '/api/onboarding/integration',
            ],
        )
        self.assertEqual(budgets, [120.0, 75.0, 74.0, 73.0, 72.0])

    def test_onboarding_stops_at_the_shared_deadline_without_repeating_user_creation(self) -> None:
        response = io.BytesIO(json.dumps({'auth_code': 'disposable-code'}).encode())
        with (
            patch.object(session.HTTP, 'open', return_value=response) as request,
            patch.object(session.time, 'monotonic', side_effect=[0.0, 120.0]),
            self.assertRaisesRegex(TimeoutError, '/auth/token exceeded the startup window'),
        ):
            session.onboard('http://127.0.0.1:18123', 'admin', deadline=120.0)
        request.assert_called_once()
        self.assertEqual(request.call_args.args[0].selector, '/api/onboarding/users')

    def test_onboarding_timeout_identifies_the_request_without_repeating_it(self) -> None:
        failure = TimeoutError('timed out')
        with (
            patch.object(session.HTTP, 'open', side_effect=failure) as request,
            patch.object(session.time, 'monotonic', return_value=0.0),
            self.assertRaisesRegex(TimeoutError, '/api/onboarding/users timed out') as stopped,
        ):
            session.onboard('http://127.0.0.1:18123', 'admin', deadline=120.0)
        request.assert_called_once()
        self.assertIs(stopped.exception.__cause__, failure)

    def test_image_admission_rejects_wrong_architecture_and_metadata(self) -> None:
        archive = self.image()
        validate_archive(archive, PLATFORMS['amd64'], '0.1.0')
        with self.assertRaisesRegex(ValueError, 'linux/arm64'):
            validate_archive(archive, PLATFORMS['aarch64'], '0.1.0')
        with self.assertRaisesRegex(ValueError, 'labels differ'):
            validate_archive(archive, PLATFORMS['amd64'], '9.0.0')

    def test_unreferenced_blobs_cannot_spoof_content_addresses(self) -> None:
        archive = self.image()
        data = b'Not the claimed content hash'
        with tarfile.open(archive, 'a') as image:
            member = tarfile.TarInfo('blobs/sha256/' + '0' * 64)
            member.size = len(data)
            image.addfile(member, io.BytesIO(data))
        with self.assertRaisesRegex(ValueError, 'blob name'):
            inspect_archive(archive)

    def test_unpacking_cannot_escape_its_directory_or_exceed_archive_limits(self) -> None:
        archive = self.root / 'unsafe.tar'
        for name in ('../escaped', '/escaped'):
            archive.write_bytes(tar_bytes({name: b'Untrusted'}))
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'regular native OCI'):
                unpack_archive(archive, self.root / name.replace('/', '_'))
        payload = b'x' * 200_000
        archive.write_bytes(gzip.compress(tar_bytes({'blobs/sha256/' + hashlib.sha256(payload).hexdigest(): payload})))
        with patch('archive_io.MAX_IMAGE_BYTES', 64 * 1024), self.assertRaisesRegex(ValueError, 'size limit'):
            unpack_archive(archive, self.root / 'limited')

    def test_remote_candidate_requires_anonymous_branch_url_and_independent_digest(self) -> None:
        archive = self.image()
        for url, digest, message in (
            ('https://example.invalid/repository', 'sha256:' + '1' * 64, 'branch suffix'),
            ('https://user:secret@example.invalid/repository#candidate', 'sha256:' + '1' * 64, 'anonymous'),
            ('https://example.invalid/repository#candidate', None, 'expected registry digest'),
            ('https://example.invalid/repository#candidate', 'not-a-digest', 'expected registry digest'),
        ):
            with self.subTest(url=url), self.assertRaisesRegex(ValueError, message):
                vm.validate_options(vm.Options(app_archive=archive, repository_url=url, registry_digest=digest))

    def test_installed_image_binds_registry_digest_config_and_platform(self) -> None:
        config = 'sha256:' + '1' * 64
        digest = 'sha256:' + '2' * 64
        manifest = 'sha256:' + '4' * 64
        reference = 'localhost:5000/candidate:0.1.0'
        container: dict[str, object] = {'Image': manifest, 'Config': {'Image': reference}}
        image: dict[str, object] = {
            'Id': manifest,
            'Architecture': 'amd64',
            'Os': 'linux',
            'RepoDigests': ['localhost:5000/candidate@' + digest],
        }
        for change, message in (
            ('none', ''),
            ('legacy', ''),
            ('index', ''),
            ('wrong-index-platform', 'archive/configuration'),
            ('config', 'archive/configuration'),
            ('reference', 'archive/configuration'),
            ('architecture', 'platform'),
            ('digest', 'registry digest'),
        ):
            actual_container, actual_image = deepcopy(container), deepcopy(image)
            if change == 'legacy':
                actual_container['Image'] = actual_image['Id'] = config
            elif change in ('index', 'wrong-index-platform'):
                actual_container['Image'] = actual_image['Id'] = digest
                actual_container['ImageManifestDescriptor'] = {
                    'digest': manifest if change == 'index' else 'sha256:' + '3' * 64
                }
            elif change == 'config':
                actual_container['Image'] = 'sha256:' + '3' * 64
            elif change == 'reference':
                object_value(actual_container['Config'])['Image'] = 'other/image:0.1.0'
            elif change == 'architecture':
                actual_image['Architecture'] = 'arm64'
            elif change == 'digest':
                actual_image['RepoDigests'] = ['localhost:5000/candidate@sha256:' + '3' * 64]
            results = [
                subprocess.CompletedProcess([], 0, stdout=json.dumps(value))
                for value in (actual_container, actual_image)
            ]
            with (
                self.subTest(change=change),
                patch.object(vm, 'registry_manifest', return_value=manifest),
                patch.object(vm, 'run', side_effect=results),
            ):
                if not message:
                    result = vm.assert_installed_image(
                        self.root / 'key',
                        2222,
                        '12345678_garmin_toolkit',
                        reference=reference,
                        digest=digest,
                        config_digest=config,
                        architecture='amd64',
                    )
                    self.assertEqual(result['config_digest'], config)
                    self.assertEqual(result['manifest_digest'], manifest)
                else:
                    with self.assertRaisesRegex(RuntimeError, message):
                        vm.assert_installed_image(
                            self.root / 'key',
                            2222,
                            '12345678_garmin_toolkit',
                            reference=reference,
                            digest=digest,
                            config_digest=config,
                            architecture='amd64',
                        )

    def test_registry_manifest_verifies_original_bytes_config_and_unique_platform(self) -> None:
        config = 'sha256:' + '1' * 64
        raw = json.dumps({'schemaVersion': 2, 'config': {'digest': config}}).encode()
        digest = 'sha256:' + hashlib.sha256(raw).hexdigest()
        manifest: dict[str, object] = {
            'Descriptor': {'digest': digest, 'platform': {'architecture': 'amd64', 'os': 'linux'}},
            'Raw': base64.b64encode(raw).decode(),
        }
        for change, message in (
            ('none', ''),
            ('index', ''),
            ('bytes', 'manifest bytes'),
            ('config', 'archive configuration'),
            ('digest', 'independently expected digest'),
            ('platform', 'expected platform'),
            ('duplicate', 'exactly one manifest'),
        ):
            actual = deepcopy(manifest)
            expected_config, expected_digest = config, digest
            if change == 'bytes':
                actual['Raw'] = base64.b64encode(raw + b' ').decode()
            elif change == 'config':
                expected_config = 'sha256:' + '3' * 64
            elif change == 'digest':
                expected_digest = 'sha256:' + '3' * 64
            elif change == 'platform':
                object_value(actual['Descriptor'])['platform'] = {'architecture': 'arm64', 'os': 'linux'}
            document = [actual, actual] if change == 'duplicate' else [actual] if change == 'index' else actual
            with (
                self.subTest(change=change),
                patch.object(vm, 'run', return_value=subprocess.CompletedProcess([], 0, stdout=json.dumps(document))),
            ):
                if message:
                    with self.assertRaisesRegex(RuntimeError, message):
                        vm.registry_manifest(
                            self.root / 'key',
                            2222,
                            'localhost:5000/candidate',
                            expected_digest,
                            expected_config,
                            'amd64',
                        )
                else:
                    self.assertEqual(
                        vm.registry_manifest(
                            self.root / 'key',
                            2222,
                            'localhost:5000/candidate',
                            expected_digest,
                            expected_config,
                            'amd64',
                        ),
                        digest,
                    )

    def test_store_timeout_checks_discovery_then_retries(self) -> None:
        slug = '12345678_garmin_toolkit'
        for collection in ('apps', 'addons'):
            with (
                self.subTest(collection=collection),
                patch.object(
                    vm,
                    'run',
                    side_effect=[
                        RuntimeError('context deadline exceeded'),
                        subprocess.CompletedProcess(
                            [], 0, stdout=json.dumps({'result': 'ok', 'data': {collection: []}})
                        ),
                        subprocess.CompletedProcess([], 0),
                        subprocess.CompletedProcess(
                            [], 0, stdout=json.dumps({'result': 'ok', 'data': {collection: [{'slug': slug}]}})
                        ),
                    ],
                ) as run,
                patch('haos_app_vm.time.sleep'),
            ):
                vm.reload_store(self.root / 'key', 2222, slug)
            self.assertEqual(run.call_count, 4)

    def test_withdrawal_refresh_waits_until_the_app_leaves_the_store(self) -> None:
        slug = '12345678_garmin_toolkit'
        with (
            patch.object(vm, 'run'),
            patch.object(vm, 'guest_data', side_effect=[{'apps': [{'slug': slug}]}, {'apps': []}]) as info,
            patch('haos_app_vm.time.sleep'),
        ):
            vm.reload_store(self.root / 'key', 2222, slug, present=False)
        self.assertEqual(info.call_count, 2)
        with (
            patch.object(vm, 'run'),
            patch.object(vm, 'guest_data', return_value={'apps': [{'slug': slug}]}),
            patch('haos_app_vm.time.sleep'),
            self.assertRaisesRegex(RuntimeError, 'expected app visibility'),
        ):
            vm.reload_store(self.root / 'key', 2222, slug, present=False)

    def test_store_refresh_waits_for_the_advertised_version_even_when_the_installed_version_matches(self) -> None:
        slug = '12345678_garmin_toolkit'
        for collection in ('apps', 'addons'):
            with (
                self.subTest(collection=collection),
                patch.object(vm, 'run', side_effect=[RuntimeError('context deadline exceeded'), None]) as reload,
                patch.object(
                    vm,
                    'guest_data',
                    side_effect=[
                        {collection: [{'slug': slug, 'version': '0.1.2', 'version_latest': '0.0.0'}]},
                        {collection: [{'slug': slug, 'version': '0.0.0', 'version_latest': '0.1.2'}]},
                    ],
                ) as info,
                patch('haos_app_vm.time.sleep'),
            ):
                vm.reload_store(self.root / 'key', 2222, slug, version='0.1.2')
            self.assertEqual(reload.call_count, 2)
            self.assertEqual(info.call_count, 2)

    def test_store_refresh_keeps_wrong_or_missing_versions_fatal_within_the_existing_budget(self) -> None:
        slug = '12345678_garmin_toolkit'
        for latest in ('0.0.0', None):
            with (
                self.subTest(latest=latest),
                patch.object(vm, 'run') as reload,
                patch.object(
                    vm,
                    'guest_data',
                    return_value={'apps': [{'slug': slug, 'version': '0.1.2', 'version_latest': latest}]},
                ),
                patch('haos_app_vm.time.sleep'),
                self.assertRaisesRegex(RuntimeError, 'expected app visibility or version'),
            ):
                vm.reload_store(self.root / 'key', 2222, slug, version='0.1.2')
            self.assertEqual(reload.call_count, 3)
        with patch.object(vm, 'run') as reload, self.assertRaises(ValueError):
            vm.reload_store(self.root / 'key', 2222, slug, present=False, version='0.1.2')
        reload.assert_not_called()

    def test_browser_forwarding_is_loopback_and_vm_resources_are_bounded(self) -> None:
        for architecture, binary in [('amd64', 'qemu-system-x86_64'), ('aarch64', 'qemu-system-aarch64')]:
            with self.subTest(architecture=architecture):
                command = vm.qemu_command(
                    self.root / 'disk',
                    self.root / 'config',
                    self.root,
                    'owned',
                    2222,
                    900,
                    guest=PLATFORMS[architecture],
                    overlay_limit=session.OVERLAY_LIMIT,
                )
                self.assertIn(binary, command)
                self.assertIn('MemoryMax=6G', command)
                self.assertIn('CPUQuota=400%', command)
                self.assertIn(f'--fsize={16 * 1024**3}', command)
                self.assertEqual(command[command.index('-smp') + 1], '4')
                self.assertEqual(command[command.index('-m') + 1], '4096')
                self.assertIn('user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22222', command)

    def test_acceptance_vm_is_bound_to_its_assigned_owner(self) -> None:
        owner = 'garmin-acceptance-' + 'a' * 32 + '.scope'
        unit = vm_scope(owner).removesuffix('.scope')
        with patch.dict(os.environ, {ACCEPTANCE_SCOPE: owner}):
            command = vm.qemu_command(
                self.root / 'disk',
                self.root / 'config',
                self.root,
                unit,
                2222,
                900,
                guest=PLATFORMS['amd64'],
                overlay_limit=session.OVERLAY_LIMIT,
            )
            self.assertIn('--unit=' + unit, command)
            self.assertIn('BindsTo=' + owner, command)
            self.assertIn('After=' + owner, command)
            self.assertIn('MemoryMax=6G', command)
            self.assertIn('CPUQuota=400%', command)
            with self.assertRaisesRegex(ValueError, 'assigned owned scope'):
                vm.qemu_command(
                    self.root / 'disk',
                    self.root / 'config',
                    self.root,
                    'unrelated',
                    2222,
                    900,
                    guest=PLATFORMS['amd64'],
                    overlay_limit=session.OVERLAY_LIMIT,
                )

    def test_haos_reboot_waits_for_a_different_boot_id_after_ssh_disconnect(self) -> None:
        before, after = '11111111-1111-4111-8111-111111111111', '22222222-2222-4222-8222-222222222222'
        (self.root / 'overlay.qcow2').write_bytes(b'synthetic-overlay')
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        with (
            patch.object(vm, 'run', return_value=subprocess.CompletedProcess([], 0, stdout=before)),
            patch.object(
                vm.subprocess,
                'run',
                side_effect=[
                    subprocess.CompletedProcess([], 255),
                    subprocess.CompletedProcess([], 0, stdout=before),
                    subprocess.TimeoutExpired('ssh', 8),
                    subprocess.CompletedProcess([], 0, stdout=after),
                ],
            ),
            patch.object(vm.time, 'sleep'),
            patch.object(vm, 'wait_for_supervisor') as ready,
        ):
            self.assertEqual(
                vm.reboot_guest(process, self.root, self.root / 'key', 2222, timeout=30, overlay_limit=1024),
                {'before': before, 'after': after},
            )
            ready.assert_called_once()

    def test_haos_reboot_cannot_pass_if_the_kernel_boot_id_does_not_change(self) -> None:
        before = '11111111-1111-4111-8111-111111111111'
        (self.root / 'overlay.qcow2').write_bytes(b'synthetic-overlay')
        process = Mock(spec=subprocess.Popen)
        process.poll.return_value = None
        with (
            patch.object(vm, 'run', return_value=subprocess.CompletedProcess([], 0, stdout=before)),
            patch.object(vm.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, stdout=before)),
            patch.object(vm.time, 'monotonic', side_effect=[0, 0, 31]),
            patch.object(vm.time, 'sleep'),
            patch.object(vm, 'wait_for_supervisor') as ready,
            self.assertRaisesRegex(TimeoutError, 'boot ID did not change'),
        ):
            vm.reboot_guest(process, self.root, self.root / 'key', 2222, timeout=30, overlay_limit=1024)
        ready.assert_not_called()

    def test_stop_refuses_reused_process_identity(self) -> None:
        directory = self.root / 'haos-session.aabbccddeeff.test'
        directory.mkdir()
        session.save_state(
            directory / 'session.json',
            {
                'pid': os.getpid(),
                'process_identity': 'wrong',
                'base_url': 'http://127.0.0.1:1',
            },
        )
        with patch.object(vm, 'SCRATCH_ROOT', self.root), patch('haos_app_session.os.kill') as kill:
            with self.assertRaisesRegex(RuntimeError, 'identity'):
                session.stop_session('aabbccddeeff')
            kill.assert_not_called()

    def test_explicit_stop_waits_for_real_runner_cleanup(self) -> None:
        for owned in (False, True):
            with self.subTest(owned=owned):
                self.check_session_stop(owned=owned)

    def check_session_stop(self, *, owned: bool) -> None:
        directory = (self.root / 'a.owned' if owned else self.root) / 'haos-session.aabbccddeeff.test'
        directory.mkdir(parents=True)
        child_code = (
            'import pathlib,signal,shutil,time,sys; root=pathlib.Path(sys.argv[1]); '
            'signal.signal(signal.SIGTERM,lambda *_: (shutil.rmtree(root),sys.exit(0))); '
            'print("ready",flush=True); time.sleep(60)'
        )
        with subprocess.Popen([sys.executable, '-c', child_code, str(directory)], stdout=subprocess.PIPE) as child:
            try:
                assert child.stdout is not None
                self.assertEqual(child.stdout.readline(), b'ready\n')
                session.save_state(
                    directory / 'session.json',
                    {
                        'pid': child.pid,
                        'process_identity': session.process_identity(child.pid),
                        'base_url': f'http://127.0.0.1:{vm.free_port()}',
                    },
                )
                with patch.object(vm, 'SCRATCH_ROOT', self.root):
                    self.assertEqual(session.stop_session('aabbccddeeff'), 0)
                self.assertFalse(directory.exists())
                self.assertEqual(child.wait(timeout=3), 0)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def test_signal_and_failure_restore_handler_and_exit_codes(self) -> None:
        options = session.Run(app_archive=Path('not-used'))
        previous = signal.getsignal(signal.SIGTERM)
        with patch('haos_app_session.run_session', side_effect=session.SessionStopped):
            self.assertEqual(session.main(options), 0)
        self.assertEqual(signal.getsignal(signal.SIGTERM), previous)
        with patch('haos_app_session.run_session', side_effect=subprocess.TimeoutExpired('owned', 1)):
            self.assertEqual(session.main(options), 124)
        self.assertEqual(signal.getsignal(signal.SIGTERM), previous)

    def test_populated_workflow_has_one_bounded_deadline_after_ingress_and_cleans_up_on_expiry(self) -> None:
        for expire in (False, True):
            with self.subTest(expire=expire):
                self.check_workflow_deadline(expire=expire)

    def check_workflow_deadline(self, *, expire: bool) -> None:
        clock = [0.0]
        lifetimes: list[int] = []
        process, browser, tunnel = (Mock(spec=subprocess.Popen) for _ in range(3))
        options = session.Check(app_archive=Path('not-used'), evidence_directory=self.root / str(expire), timeout=60)

        def boot(
            _options: vm.Options,
            runtime: Path,
            _image: Path,
            *,
            on_started: Callable[[subprocess.Popen[str], Path, int, str], None],
            on_boot: object,
            lifetime: int,
            overlay_limit: int,
        ) -> None:
            lifetimes.append(lifetime)
            (runtime / 'installation.json').write_text(
                json.dumps(
                    {
                        'image_reference': 'disposable',
                        'registry_digest': 'sha256:' + 'a' * 64,
                        'config_digest': 'sha256:' + 'b' * 64,
                        'architecture': 'amd64',
                    }
                )
            )
            on_started(process, runtime / 'key', 2222, 'local_garmin_toolkit')

        def ready(_browser: object, _evidence: Path, checkpoint: str = 'ready.json') -> None:
            if checkpoint == 'ready.json':
                self.assertEqual(timer.call_args_list, [call(signal.ITIMER_REAL, 60)])
                clock[0] = 55.0

        def exercise(*_args: object, reboot: Callable[[], dict[str, str]], **_kwargs: object) -> None:
            self.assertEqual(timer.call_args_list, [call(signal.ITIMER_REAL, 60), call(signal.ITIMER_REAL, 60)])
            for elapsed in (70.0, 90.0):
                clock[0] = elapsed
                reboot()
                self.assertEqual(core.call_args.args[3], 115.0)
            self.assertEqual(timer.call_args_list, [call(signal.ITIMER_REAL, 60), call(signal.ITIMER_REAL, 60)])
            if expire:
                session.workflow_expired(signal.SIGALRM, None)

        with (
            patch.object(vm, 'SCRATCH_ROOT', self.root),
            patch.object(vm, 'validate_options', return_value=PLATFORMS['amd64']),
            patch.object(vm, 'prepare_runtime', return_value=self.root / 'image'),
            patch.object(vm, 'boot', side_effect=boot),
            patch.object(vm, 'guest_data', return_value={}),
            patch.object(vm, 'reboot_guest', return_value={'before': 'one', 'after': 'two'}),
            patch.object(vm, 'assert_started'),
            patch.object(vm, 'assert_installed_image'),
            patch.object(session, 'launch_browser', return_value=browser),
            patch.object(session, 'launch_tunnel', return_value=tunnel),
            patch.object(session, 'wait_browser', side_effect=ready),
            patch.object(session, 'wait_for_core') as core,
            patch.object(session, 'onboard', return_value={}),
            patch.object(session, 'stop_group') as stop,
            patch.object(session.haos_workflow, 'exercise', side_effect=exercise),
            patch.object(session.time, 'monotonic', side_effect=lambda: clock[0]),
            patch.object(session.signal, 'signal', return_value=signal.SIG_DFL),
            patch.object(session.signal, 'setitimer') as timer,
        ):
            self.assertEqual(session.main(options), 124 if expire else 0)
        self.assertEqual(lifetimes, [150])
        self.assertEqual(
            timer.call_args_list,
            [call(signal.ITIMER_REAL, 60), call(signal.ITIMER_REAL, 60), call(signal.ITIMER_REAL, 0)],
        )
        stop.assert_any_call(browser)
        stop.assert_any_call(tunnel)
        self.assertEqual(list(self.root.glob('haos-session.*')), [])
        self.assertEqual(list(self.root.glob('c.*')), [])

    def test_startup_failure_removes_session_scratch(self) -> None:
        options = session.Run(app_archive=Path('not-used'), verify_browser=False)
        with (
            patch.object(vm, 'SCRATCH_ROOT', self.root),
            patch.object(vm, 'validate_options', return_value=PLATFORMS['amd64']),
            patch.object(vm, 'prepare_runtime', side_effect=RuntimeError('startup failed')),
            self.assertRaisesRegex(RuntimeError, 'startup failed'),
        ):
            session.run_session(options)
        self.assertEqual(list(self.root.glob('haos-session.*')), [])
        self.assertEqual(list(self.root.glob('c.*')), [])

    def test_owned_evidence_cannot_be_written_into_disposable_scratch(self) -> None:
        scratch = self.root / 'a.owned'
        scratch.mkdir()
        options = session.Check(app_archive=Path('unused'), evidence_directory=scratch / 'evidence')
        with (
            patch.object(vm, 'SCRATCH_ROOT', self.root),
            patch.object(vm, 'validate_options', return_value=PLATFORMS['amd64']),
            patch.dict(
                os.environ,
                {
                    'GARMIN_ACCEPTANCE_SCOPE': 'garmin-acceptance-' + 'a' * 32 + '.scope',
                    'GARMIN_ACCEPTANCE_SCRATCH': str(scratch),
                },
            ),
            self.assertRaisesRegex(ValueError, 'outside disposable scratch'),
        ):
            session.run_session(options)
        self.assertFalse((scratch / 'evidence').exists())

    def test_browser_preflight_failure_stops_child_without_booting_vm(self) -> None:
        options = session.Run(app_archive=Path('not-used'))
        with subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'], start_new_session=True) as browser:
            try:
                with (
                    patch.object(vm, 'SCRATCH_ROOT', self.root),
                    patch.object(vm, 'validate_options', return_value=PLATFORMS['amd64']),
                    patch.object(vm, 'prepare_runtime') as prepare,
                    patch('haos_app_session.launch_browser', return_value=browser),
                    patch('haos_app_session.wait_browser', side_effect=RuntimeError('browser failed')),
                    self.assertRaisesRegex(RuntimeError, 'browser failed'),
                ):
                    session.run_session(options)
                prepare.assert_not_called()
                self.assertIsNotNone(browser.poll())
                self.assertEqual(list(self.root.glob('haos-session.*')), [])
                self.assertEqual(list(self.root.glob('c.*')), [])
            finally:
                if browser.poll() is None:
                    browser.kill()
                    browser.wait(timeout=3)


if __name__ == '__main__':
    unittest.main()
