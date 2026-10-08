import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from cli_output import TerminalOutput
from hass_integration import browser_progress, entrypoint, run_command, stage, stop


@unittest.skipUnless(sys.platform == 'linux', 'The VM runner requires Linux process groups')
class IntegrationRunnerTests(unittest.TestCase):
    def test_terminal_colors_are_captured_while_machine_stdout_stays_separate(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            run_command(
                [
                    sys.executable,
                    '-c',
                    'import os,sys; assert os.isatty(2); assert os.environ["FORCE_COLOR"] == "1"; '
                    'assert "NO_COLOR" not in os.environ; print("/driver"); '
                    'print("\\x1b[31mfailure\\x1b[0m", file=sys.stderr)',
                ],
                root / 'build.log',
                stdout=root / 'driver-path',
                environment={**os.environ, 'NO_COLOR': '1', 'FORCE_COLOR': '0', 'TERM': 'dumb'},
                timeout=5,
            )
            self.assertEqual((root / 'driver-path').read_bytes(), b'/driver\n')
            self.assertEqual((root / 'build.log').read_bytes(), b'\x1b[31mfailure\x1b[0m\n')

    def test_build_stdout_is_separate_from_diagnostics(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            run_command(
                [sys.executable, '-c', 'import sys; print("/driver"); print("build log", file=sys.stderr)'],
                root / 'build.log',
                stdout=root / 'driver-path',
                timeout=5,
            )
            self.assertEqual((root / 'driver-path').read_text(), '/driver\n')
            self.assertEqual((root / 'build.log').read_text(), 'build log\n')

    def test_failure_preserves_diagnostics_and_exit_code(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'vm.log'
            with self.assertRaises(subprocess.CalledProcessError) as caught:
                run_command([sys.executable, '-c', 'print("assertion failed"); raise SystemExit(7)'], log, timeout=5)
            self.assertEqual(caught.exception.returncode, 7)
            self.assertEqual(log.read_text(), 'assertion failed\n')

    def test_live_output_reaches_the_caller_before_the_child_exits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            acknowledgement = root / 'acknowledged'
            received = bytearray()

            def acknowledge(data: bytes) -> None:
                received.extend(data)
                if b'ready' in received:
                    acknowledgement.touch()

            run_command(
                [
                    sys.executable,
                    '-c',
                    'import sys, time\n'
                    'from pathlib import Path\n'
                    'print("ready", flush=True)\n'
                    'while not Path(sys.argv[1]).exists():\n'
                    '    time.sleep(0.01)\n'
                    'print("done", flush=True)\n',
                    str(acknowledgement),
                ],
                root / 'vm.log',
                timeout=5,
                live=acknowledge,
            )
            self.assertEqual((root / 'vm.log').read_text(), 'ready\ndone\n')

    def test_timeout_stops_and_reaps_child(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'vm.log'
            with self.assertRaises(subprocess.TimeoutExpired):
                run_command(
                    [sys.executable, '-c', 'import os,time; print(os.getpid(), flush=True); time.sleep(60)'],
                    log,
                    timeout=0.5,
                )
            pid = int(log.read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            with self.assertRaises(ChildProcessError):
                os.waitpid(pid, os.WNOHANG)

    def test_timeout_retains_output_emitted_during_shutdown(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'vm.log'
            with self.assertRaises(subprocess.TimeoutExpired):
                run_command(
                    [
                        sys.executable,
                        '-c',
                        'import signal,sys,time; '
                        'signal.signal(signal.SIGTERM, lambda *_: '
                        '(print("shutdown complete", flush=True), sys.exit(0))); '
                        'print("ready", flush=True); time.sleep(60)',
                    ],
                    log,
                    timeout=0.5,
                )
            self.assertEqual(log.read_text(), 'ready\nshutdown complete\n')

    def test_cleanup_escalates_when_child_ignores_termination(self) -> None:
        with subprocess.Popen(
            [
                sys.executable,
                '-c',
                'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); '
                'print("ready", flush=True); time.sleep(60)',
            ],
            stdout=subprocess.PIPE,
            start_new_session=True,
        ) as child:
            try:
                assert child.stdout is not None
                self.assertEqual(child.stdout.readline(), b'ready\n')
                stop(child, grace=0.05)
                self.assertEqual(child.returncode, -signal.SIGKILL)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def test_interruption_still_runs_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'vm.log'
            with subprocess.Popen(
                [
                    sys.executable,
                    '-c',
                    'import sys; from pathlib import Path; from hass_integration import run_command; '
                    'run_command([sys.executable, "-c", '
                    '"import os,time; print(os.getpid(), flush=True); time.sleep(60)"], '
                    'Path(sys.argv[1]), timeout=60)',
                    str(log),
                ],
                cwd=Path(__file__).parent,
                stderr=subprocess.PIPE,
            ) as runner:
                try:
                    deadline = time.monotonic() + 5
                    while not log.exists() or not log.read_text().strip():
                        self.assertLess(time.monotonic(), deadline, 'Runner did not start its child')
                        time.sleep(0.01)
                    pid = int(log.read_text())
                    runner.send_signal(signal.SIGINT)
                    runner.communicate(timeout=5)
                    self.assertNotEqual(runner.returncode, 0)
                    with self.assertRaises(ProcessLookupError):
                        os.kill(pid, 0)
                finally:
                    if runner.poll() is None:
                        runner.send_signal(signal.SIGINT)
                        runner.communicate(timeout=15)


class RunnerPresentationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.output = StringIO()
        self.errors = StringIO()
        previous = sys.excepthook
        self.addCleanup(setattr, sys, 'excepthook', previous)
        terminal = TerminalOutput(stdout=self.output, stderr=self.errors, environment={})
        self.patch = patch('hass_integration.OUTPUT', terminal)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def test_expected_errors_use_panels_and_correct_exit_codes(self) -> None:
        cases = [
            (subprocess.CalledProcessError(7, ['child']), 7),
            (subprocess.CalledProcessError(-signal.SIGTERM, ['child']), 143),
            (subprocess.TimeoutExpired(['child'], 5), 124),
            (FileNotFoundError('executable missing'), 127),
            (PermissionError('access denied'), 126),
            (OSError('disk full'), 1),
            (KeyboardInterrupt(), 130),
        ]
        for error, code in cases:
            with self.subTest(error=error):
                self.errors.seek(0)
                self.errors.truncate()
                with patch('hass_integration.main', side_effect=error):
                    self.assertEqual(entrypoint([]), code)
                rendered = self.errors.getvalue()
                self.assertIn('╭', rendered)
                self.assertNotIn('Traceback', rendered)
                self.assertNotIn('\x1b', rendered)

    def test_stage_shows_live_output_and_keeps_the_original_log(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'vm.log'
            stage(
                'Browser assertions',
                [
                    sys.executable,
                    '-c',
                    'import sys; sys.stdout.buffer.write(b"\\x1b[31mfirst\\x1b[0m\\nsecond\\n"); sys.stdout.flush()',
                ],
                log,
                timeout=5,
            )
            rendered = self.output.getvalue()
            self.assertIn('first', rendered)
            self.assertIn('second', rendered)
            self.assertIn(str(log), rendered)
            self.assertNotIn('\x1b', rendered)
            self.assertEqual(log.read_bytes(), b'\x1b[31mfirst\x1b[0m\nsecond\n')

    def test_vm_progress_omits_boot_noise_but_retains_it_in_the_log(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'vm.log'
            stage(
                'Browser assertions',
                [
                    sys.executable,
                    '-c',
                    'print("machine # [1.0] Xvfb[1]: warning"); '
                    'print("machine # [2.0] hass-integration[1]: ok 1 - route import")',
                ],
                log,
                timeout=5,
                live_filter=browser_progress,
            )
            self.assertIn('ok 1 - route import', self.output.getvalue())
            self.assertNotIn('\nmachine # [1.0] Xvfb[1]: warning\n', self.output.getvalue())
            self.assertIn('Xvfb[1]', log.read_text())

    def test_tyro_help_and_usage_errors_are_rendered_through_rich(self) -> None:
        self.assertEqual(entrypoint(['--help']), 0)
        self.assertIn('--driver', self.output.getvalue())
        self.assertEqual(entrypoint(['--invalid-option']), 2)
        self.assertIn('Usage error', self.errors.getvalue())
        self.assertNotIn('Traceback', self.errors.getvalue())

    def test_reconnect_suite_is_selected_through_the_cli(self) -> None:
        with patch('hass_integration.main', return_value=0) as run:
            self.assertEqual(entrypoint(['--suite', 'reconnect']), 0)
            run.assert_called_once_with(None, 'reconnect')

    def test_unexpected_errors_reach_the_installed_rich_crash_handler(self) -> None:
        error = RuntimeError('unexpected runner defect')
        with patch('hass_integration.main', side_effect=error):
            try:
                entrypoint([])
            except RuntimeError as caught:
                sys.excepthook(type(caught), caught, caught.__traceback__)
            else:
                self.fail('Unexpected exceptions must reach the crash handler')
        rendered = self.errors.getvalue()
        self.assertIn('╭', rendered)
        self.assertIn('unexpected runner defect', rendered)
        self.assertIn('Traceback', rendered)
