"""Acceptance cleanup must reclaim detached children and its independent VM scope."""

import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import suppress
from pathlib import Path
from unittest.mock import patch

import haos_app_session as session
from haos_platform import PLATFORMS
from json_data import json_bytes, object_value, read_json, text
from processes import ACCEPTANCE_SCRATCH, run_owned, session_scratch, stop_group, stop_scopes, vm_scope
from test_hass_ci import USER_MANAGER

ROOT = Path(__file__).resolve().parents[2]


def session_fixture(marker: Path, outcome: str, controller: int) -> None:
    """Exercise real session allocation with synthetic browser/VM processes only."""
    scratch = Path(os.environ[ACCEPTANCE_SCRATCH])
    evidence = marker.with_suffix('.evidence')
    pids = [os.getpid()]
    state: dict[str, object] = {}
    sleeper = 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)'

    def record() -> None:
        marker.write_bytes(json_bytes({**state, 'pids': pids, 'owner': os.environ['GARMIN_ACCEPTANCE_SCOPE']}))

    def browser(state_path: Path, _evidence: Path) -> subprocess.Popen[str]:
        state.update(object_value(read_json(state_path)))
        process = subprocess.Popen([sys.executable, '-c', sleeper], start_new_session=True, text=True)
        pids.append(process.pid)
        (Path(text(state['browser_temporary'])) / 'browser-cache').write_text('temporary browser data')
        return process

    def prepare(_options: object, runtime: Path, _guest: object) -> Path:
        state.update(scratch=str(scratch), runtime=str(runtime))
        (runtime / 'overlay.qcow2').write_bytes(b'synthetic overlay')
        (runtime / 'outside-link').symlink_to(marker.parent, target_is_directory=True)
        with tempfile.NamedTemporaryFile(delete=False) as implicit:
            implicit.write(b'implicit temporary data')
            state['implicit'] = implicit.name
            assert Path(implicit.name).is_relative_to(scratch)
        (evidence / 'retained.json').write_bytes(json_bytes({'outcome': outcome}))
        record()
        if outcome == 'error':
            raise RuntimeError('synthetic startup failure')
        return runtime / 'overlay.qcow2'

    def boot(_options: object, _runtime: Path, _image: Path, **_callbacks: object) -> None:
        owner = os.environ['GARMIN_ACCEPTANCE_SCOPE']
        vm_marker = marker.with_suffix('.vm')
        program = f'import os,pathlib; pathlib.Path({str(vm_marker)!r}).write_text(str(os.getpid())); ' + sleeper
        process = subprocess.Popen(
            [
                'systemd-run',
                '--user',
                '--scope',
                '--quiet',
                '--collect',
                '--expand-environment=no',
                '--unit=' + vm_scope(owner),
                '-p',
                'BindsTo=' + owner,
                '-p',
                'After=' + owner,
                '-p',
                'MemoryMax=64M',
                '-p',
                'CPUQuota=100%',
                '-p',
                'RuntimeMaxSec=15',
                '-p',
                'TimeoutStopSec=1s',
                sys.executable,
                '-c',
                program,
            ],
            start_new_session=True,
        )
        pids.append(process.pid)
        record()
        while not vm_marker.exists():
            time.sleep(0.01)
        if outcome == 'crash':
            os._exit(7)
        if outcome in ('timeout', 'cancel'):
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if outcome == 'cancel':
            os.kill(controller, signal.SIGTERM)
        if outcome in ('timeout', 'cancel'):
            time.sleep(60)

    options = session.Check(app_archive=marker.parent / 'unused.oci.tar', evidence_directory=evidence, timeout=10)

    def stop_browser(process: subprocess.Popen[str]) -> None:
        stop_group(process, grace=0.05)

    with (
        patch.object(session.vm, 'validate_options', return_value=PLATFORMS['amd64']),
        patch.object(session.vm, 'prepare_runtime', side_effect=prepare),
        patch.object(session.vm, 'boot', side_effect=boot),
        patch.object(session, 'launch_browser', side_effect=browser),
        patch.object(session, 'wait_browser'),
        patch.object(session, 'stop_group', side_effect=stop_browser),
    ):
        status = session.main(options)
        if status:
            raise SystemExit(status)


class ProcessTests(unittest.TestCase):
    def test_scope_names_cannot_target_an_unrelated_unit(self) -> None:
        owner = 'garmin-acceptance-' + 'a' * 32 + '.scope'
        self.assertEqual(vm_scope(owner), owner.removesuffix('.scope') + '-vm.scope')
        for invalid in ('ssh.service', '*.scope', 'garmin-acceptance-x.scope', owner + '/other'):
            with self.subTest(owner=invalid), self.assertRaises(ValueError):
                stop_scopes(invalid)

    def test_scratch_is_removed_after_launch_failure_but_retained_if_scopes_are_unverified(self) -> None:
        with tempfile.TemporaryDirectory(prefix='process-tests.', dir=ROOT / '.tmp') as temporary:
            base = Path(temporary)
            for cleanup_fails in (False, True):
                with (
                    self.subTest(cleanup_fails=cleanup_fails),
                    patch('processes.Path.cwd', return_value=base),
                    patch('processes.subprocess.Popen', side_effect=OSError('launch failed')),
                    patch('processes.stop_scopes', side_effect=RuntimeError('not empty') if cleanup_fails else None),
                    self.assertRaisesRegex(
                        RuntimeError if cleanup_fails else OSError, 'not empty' if cleanup_fails else 'launch failed'
                    ),
                ):
                    run_owned('unused')
                self.assertEqual(bool(list((base / '.tmp').glob('a.*'))), cleanup_fails)

    def test_session_scratch_rejects_a_directory_outside_the_workspace(self) -> None:
        with tempfile.TemporaryDirectory(prefix='process-tests.', dir=ROOT / '.tmp') as temporary:
            base = Path(temporary) / 'workspace'
            base.mkdir()
            outside = Path(temporary) / 'a.outside'
            outside.mkdir()
            with (
                patch.dict(
                    os.environ,
                    {
                        'GARMIN_ACCEPTANCE_SCOPE': 'garmin-acceptance-' + 'a' * 32 + '.scope',
                        ACCEPTANCE_SCRATCH: str(outside),
                    },
                ),
                self.assertRaisesRegex(ValueError, 'private directory'),
            ):
                session_scratch(base)

    def test_cleanup_does_not_hide_a_failed_manager_query(self) -> None:
        with (
            patch(
                'processes.subprocess.run',
                side_effect=[
                    subprocess.CompletedProcess([], 1),
                    subprocess.CompletedProcess([], 1),
                    subprocess.CalledProcessError(1, ['systemctl']),
                ],
            ),
            self.assertRaises(subprocess.CalledProcessError),
        ):
            stop_scopes('garmin-acceptance-' + 'a' * 32 + '.scope')

    def test_scope_state_is_checked_after_a_timed_out_kill_request(self) -> None:
        with (
            patch(
                'processes.subprocess.run',
                side_effect=[
                    subprocess.TimeoutExpired(['systemctl', 'kill'], 5),
                    subprocess.CompletedProcess([], 0),
                    subprocess.CompletedProcess([], 0, stdout='ActiveState=inactive\nControlGroup=\n'),
                ],
            ) as control,
        ):
            stop_scopes('garmin-acceptance-' + 'a' * 32 + '.scope')
        self.assertIn('stop', control.call_args_list[1].args[0])
        self.assertIn('show', control.call_args_list[2].args[0])

    def test_a_timed_out_stop_cannot_hide_a_populated_scope(self) -> None:
        with (
            patch(
                'processes.subprocess.run',
                side_effect=[
                    subprocess.CompletedProcess([], 0),
                    subprocess.TimeoutExpired(['systemctl', 'stop'], 15),
                    subprocess.CompletedProcess([], 0, stdout='ActiveState=active\n'),
                ],
            ),
            patch('processes.time.monotonic', side_effect=[0, 6]),
            self.assertRaisesRegex(RuntimeError, 'did not become empty'),
        ):
            stop_scopes('garmin-acceptance-' + 'a' * 32 + '.scope')

    @unittest.skipUnless(USER_MANAGER, 'requires a systemd user manager')
    def test_owned_scope_preserves_literal_arguments(self) -> None:
        with tempfile.TemporaryDirectory(prefix='process-tests.', dir=ROOT / '.tmp') as temporary:
            marker = Path(temporary) / 'literal-${HOME}'
            values = ['${HOME}', '$CARGO_BUILD_JOBS', '$$']
            run_owned(
                sys.executable,
                '-c',
                'import json,pathlib,sys; pathlib.Path(sys.argv[1]).write_text(json.dumps(sys.argv[2:]))',
                str(marker),
                *values,
                timeout=5,
            )
            self.assertEqual(json.loads(marker.read_text()), values)

    @unittest.skipUnless(USER_MANAGER, 'requires a systemd user manager')
    def test_complete_session_cleanup_preserves_only_evidence(self) -> None:
        with tempfile.TemporaryDirectory(prefix='process-tests.', dir=ROOT / '.tmp') as temporary:
            for outcome in ('exit', 'error', 'crash', 'timeout', 'cancel'):
                with self.subTest(outcome=outcome):
                    self.check_process_tree(Path(temporary) / outcome, outcome)

    def check_process_tree(self, marker: Path, outcome: str) -> None:
        vm_marker = marker.with_suffix('.vm')
        protected = marker.parent / 'unrelated'
        protected.write_text('keep unrelated data')
        arguments = (
            sys.executable,
            '-c',
            'import sys; from pathlib import Path; sys.path.insert(0, sys.argv[1]); '
            'from test_processes import session_fixture; '
            'session_fixture(Path(sys.argv[2]), sys.argv[3], int(sys.argv[4]))',
            str(Path(__file__).resolve().parent),
            str(marker),
            outcome,
            str(os.getpid()),
        )
        try:
            if outcome == 'exit':
                run_owned(*arguments, timeout=5, grace=0.05)
            else:
                error = (
                    SystemExit
                    if outcome == 'cancel'
                    else subprocess.TimeoutExpired
                    if outcome == 'timeout'
                    else subprocess.CalledProcessError
                )
                with self.assertRaises(error):
                    run_owned(*arguments, timeout=5, grace=0.05)
            document = json.loads(marker.read_text())
            pids = [*document['pids'], *([int(vm_marker.read_text())] if vm_marker.exists() else [])]
            for pid in pids:
                self.assert_stopped(pid)
            for key in ('scratch', 'runtime', 'browser_temporary', 'implicit'):
                self.assertFalse(Path(document[key]).exists(), f'{key} survived cleanup')
            self.assertEqual(protected.read_text(), 'keep unrelated data')
            self.assertEqual(
                json.loads((marker.with_suffix('.evidence') / 'retained.json').read_text()), {'outcome': outcome}
            )
            self.assertIsNone(self.active_unit(document['owner']))
            self.assertIsNone(self.active_unit(vm_scope(document['owner'])))
        finally:
            if marker.exists():
                document = json.loads(marker.read_text())
                stop_scopes(document['owner'])
                for pid in document['pids']:
                    with suppress(ProcessLookupError):
                        os.killpg(pid, signal.SIGKILL)

    def assert_stopped(self, pid: int) -> None:
        deadline = time.monotonic() + 5
        while Path(f'/proc/{pid}').exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertFalse(Path(f'/proc/{pid}').exists(), f'Owned process {pid} survived cleanup')

    def active_unit(self, unit: str) -> str | None:
        result = subprocess.run(
            ['systemctl', '--user', '--no-ask-password', 'show', '--property=ActiveState', '--value', unit],
            capture_output=True,
            text=True,
            check=True,
            timeout=5,
        )
        return None if result.stdout.strip() in ('inactive', 'failed') else result.stdout.strip()
