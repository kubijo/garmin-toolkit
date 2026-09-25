import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

WRAPPER = Path(__file__).resolve().parents[1] / 'just' / 'memory-capped.sh'


class BuildLimitsTests(unittest.TestCase):
    def test_macos_limits_builders_and_preserves_config_and_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            uname = Path(directory) / 'uname'
            uname.write_text('printf "Darwin\\n"\n')
            uname.chmod(0o755)
            environment = {
                **os.environ,
                'PATH': f'{directory}:{os.environ["PATH"]}',
                'CARGO_BUILD_JOBS': '8',
                'NIX_BUILD_CORES': '8',
                'NIX_CONFIG': 'sandbox = true\nmax-jobs = 8\ncores = 8',
            }
            argument = 'a path with spaces; $(false)'
            result = subprocess.run(
                [
                    'bash',
                    str(WRAPPER),
                    sys.executable,
                    '-c',
                    'import json, os, sys; print(json.dumps([dict(os.environ), sys.argv[1:]]))',
                    argument,
                ],
                env=environment,
                check=True,
                capture_output=True,
                text=True,
            )
            actual, arguments = json.loads(result.stdout)
            self.assertEqual(actual['CARGO_BUILD_JOBS'], '1')
            self.assertEqual(actual['NIX_BUILD_CORES'], '1')
            self.assertEqual(actual['NIX_CONFIG'], environment['NIX_CONFIG'] + '\nmax-jobs = 1\ncores = 1')
            self.assertEqual(arguments, [argument])

    def test_linux_retains_systemd_memory_limits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            for name, script in {
                'uname': 'printf "Linux\\n"\n',
                'systemctl': 'exit 0\n',
                'systemd-run': 'printf "%s\\n" "$@"\n',
            }.items():
                executable = Path(directory) / name
                executable.write_text(script)
                executable.chmod(0o755)
            result = subprocess.run(
                ['bash', str(WRAPPER), 'example-command', 'argument with spaces'],
                env={**os.environ, 'PATH': f'{directory}:{os.environ["PATH"]}'},
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                result.stdout.splitlines(),
                [
                    '--user',
                    '--scope',
                    '--quiet',
                    '-p',
                    'MemoryHigh=4G',
                    '-p',
                    'MemoryMax=6G',
                    '-p',
                    'MemorySwapMax=1G',
                    'example-command',
                    'argument with spaces',
                ],
            )
