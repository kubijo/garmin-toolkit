"""Timing accuracy and subprocess ownership for development build traces."""

import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from build_profile import ROOT, build_command, collect_compiler_profiles, report_html, spans

FIXTURE = ROOT / 'infra/python/fixtures/build_profile_child.py'


class BuildProfileTests(unittest.TestCase):
    def test_parallel_spans_do_not_mix_processes(self) -> None:
        events = [
            dict(name='build', ph='B', tid=1, ts=0, args={}),
            dict(name='rustc a', ph='B', tid=2, ts=10, args={}),
            dict(name='rustc b', ph='B', tid=3, ts=20, args={}),
            dict(name='rustc a', ph='E', tid=2, ts=70, args={'code': 1}),
            dict(name='rustc b', ph='E', tid=3, ts=90, args={'code': 0}),
            dict(name='build', ph='E', tid=1, ts=100, args={'code': 1}),
        ]
        result = spans(events)
        self.assertEqual(
            [(item['name'], item['dur']) for item in result],
            [
                ('rustc a', 60),
                ('rustc b', 70),
                ('build', 100),
            ],
        )
        self.assertEqual(result[0]['args']['code'], 1)

    def test_incomplete_process_is_explicit(self) -> None:
        result = spans([dict(name='killed', ph='B', tid=1, ts=0, args={})])
        self.assertTrue(result[0]['args']['incomplete'])

    def test_html_escapes_commands_and_metadata(self) -> None:
        report = report_html([dict(name='<compiler>', ts=0, dur=1000, args={'command': '<script>'})], {})
        self.assertNotIn('<script>', report)
        self.assertIn('&lt;compiler&gt;', report)

    def test_targets_only_build_and_respect_mode(self) -> None:
        for target in ('web', 'desktop', 'hass', 'gallery', 'cli'):
            command, _ = build_command(target, 'production', 'dev')
            self.assertEqual(command[1], 'build')
            self.assertNotIn('demo', command)
        self.assertIn('demo', build_command('desktop', 'demo', 'dev')[0])
        self.assertNotIn('demo', build_command('cli', 'demo', 'dev')[0])

    def test_wrapper_preserves_jobserver_descriptors(self) -> None:
        read_fd, write_fd = os.pipe()
        try:
            with tempfile.TemporaryDirectory() as temporary:
                result = subprocess.run(
                    [
                        sys.executable,
                        str(FIXTURE),
                        'descriptors',
                        '--read-fd',
                        str(read_fd),
                        '--write-fd',
                        str(write_fd),
                    ],
                    cwd=ROOT / 'infra/python',
                    env={
                        **os.environ,
                        'GARMIN_BUILD_TRACE': temporary,
                        'CARGO_MAKEFLAGS': f'--jobserver-auth={read_fd},{write_fd}',
                    },
                    pass_fds=(read_fd, write_fd),
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
        finally:
            os.close(read_fd)
            os.close(write_fd)

    def test_compiler_profile_only_instruments_requested_library(self) -> None:
        output = ROOT / '.tmp/build-profile/test/compiler'
        command, _ = build_command('desktop', 'demo', 'dev', output)
        self.assertEqual(command[1], 'rustc')
        self.assertIn('--lib', command)
        self.assertIn('demo', command)
        self.assertEqual(command[command.index('--') + 1], f'-Zself-profile={output}')
        with self.assertRaises(ValueError):
            build_command('web', 'demo', 'dev', output)

    def test_compiler_report_never_reuses_a_previous_runs_profile(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, destination = root / 'raw', root / 'report'
            source.mkdir()
            destination.mkdir()
            old = source / 'old.mm_profdata'
            old.write_bytes(b'prior invocation')
            previous = {old}
            with patch('build_profile.compiler_reports') as report:
                self.assertFalse(collect_compiler_profiles(source, destination, previous))
                report.assert_not_called()
                fresh = source / 'new.mm_profdata'
                fresh.write_bytes(b'current invocation')
                self.assertTrue(collect_compiler_profiles(source, destination, previous))
                report.assert_called_once_with(destination)
            self.assertTrue(old.exists())
            self.assertFalse(fresh.exists())
            self.assertEqual((destination / fresh.name).read_bytes(), b'current invocation')

    def test_signals_terminate_owned_process_group_and_keep_trace(self) -> None:
        for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            with self.subTest(signal=sig), tempfile.TemporaryDirectory() as temporary:
                ready = Path(temporary) / 'ready'
                with subprocess.Popen(
                    [sys.executable, str(FIXTURE), 'signal', '--ready', str(ready)],
                    cwd=ROOT / 'infra/python',
                    env={**os.environ, 'GARMIN_BUILD_TRACE': temporary},
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                ) as process:
                    try:
                        deadline = time.monotonic() + 5
                        while not ready.exists() and time.monotonic() < deadline:
                            time.sleep(0.01)
                        self.assertTrue(ready.exists())
                        process.send_signal(sig)
                        self.assertEqual(process.wait(timeout=5), 128 + sig)
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait()
                events = [
                    json.loads(line)
                    for file in Path(temporary).glob('*.jsonl')
                    for line in file.read_text().splitlines()
                ]
                self.assertEqual(spans(events)[0]['args']['code'], 128 + sig)


if __name__ == '__main__':
    unittest.main()
