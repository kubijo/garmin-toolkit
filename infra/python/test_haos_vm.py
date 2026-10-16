"""Exercise image admission, native selection, and owned-session cleanup."""

import os
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

import haos_app_session as session
import haos_app_vm as vm
from fixtures.images import write_image
from haos_platform import PLATFORMS, acceleration, resolve, validate_archive


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

    def test_auto_matches_native_architecture_and_rejects_unknown_host(self) -> None:
        for machine, expected in [('x86_64', 'amd64'), ('aarch64', 'aarch64'), ('arm64', 'aarch64')]:
            with self.subTest(machine=machine), patch('haos_platform.platform.machine', return_value=machine):
                self.assertEqual(resolve().architecture, expected)
        with patch('haos_platform.platform.machine', return_value='unknown'), self.assertRaises(ValueError):
            resolve()

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
        with (
            patch.object(vm, 'guest_data', return_value={'port': 80, 'ssl': False}),
            patch('haos_app_session.subprocess.Popen') as start,
        ):
            session.launch_tunnel(self.root / 'key', 2222, 18123, self.root, float('inf'))
        command = start.call_args.args[0]
        self.assertIn('127.0.0.1:18123:127.0.0.1:80', command)
        self.assertIn('ExitOnForwardFailure=yes', command)
        self.assertIn('IdentityAgent=none', command)
        self.assertTrue(start.call_args.kwargs['start_new_session'])

    def test_http_tunnel_waits_for_core_to_replace_landing_page(self) -> None:
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
            session.launch_tunnel(self.root / 'key', 2222, 18123, self.root, float('inf'))
        self.assertIn('127.0.0.1:18123:127.0.0.1:80', start.call_args.args[0])

    def test_image_admission_rejects_wrong_architecture_and_metadata(self) -> None:
        archive = self.image()
        validate_archive(archive, PLATFORMS['amd64'], '0.1.0')
        with self.assertRaisesRegex(ValueError, 'linux/arm64'):
            validate_archive(archive, PLATFORMS['aarch64'], '0.1.0')
        with self.assertRaisesRegex(ValueError, 'labels differ'):
            validate_archive(archive, PLATFORMS['amd64'], '9.0.0')

    def test_store_timeout_checks_discovery_then_retries(self) -> None:
        with (
            patch.object(
                vm,
                'run',
                side_effect=[
                    RuntimeError('context deadline exceeded'),
                    subprocess.CompletedProcess([], 0, stdout='{"apps":[]}'),
                    subprocess.CompletedProcess([], 0),
                    subprocess.CompletedProcess([], 0, stdout='{"apps":[{"slug":"local_garmin_toolkit"}]}'),
                ],
            ) as run,
            patch('haos_app_vm.time.sleep'),
        ):
            vm.reload_store(self.root / 'key', 2222)
        self.assertEqual(run.call_count, 4)

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
        directory = self.root / 'haos-session.aabbccddeeff.test'
        directory.mkdir()
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
