import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

WRAPPER = Path(__file__).resolve().parents[1] / 'just' / 'memory-capped.sh'


class BuildLimitsTests(unittest.TestCase):
    def test_gallery_checks_cannot_replace_live_scene_library(self) -> None:
        just = shutil.which('just')
        assert just is not None
        root = WRAPPER.parents[2]
        command = [just, '--justfile', str(root / 'infra/gallery/justfile')]
        environment = {**os.environ, 'CARGO_TARGET_DIR': str(root / '.tmp/gallery-target')}
        evaluated = subprocess.run(
            [*command, '--evaluate', 'CARGO_TARGET_DIR'],
            env=environment,
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(evaluated.stdout.strip(), str(root / '.tmp/gallery-check-target'))
        for recipe in (
            ['hot'],
            ['run', '--hot'],
            ['capture'],
            ['render', 'scene', 'out.png'],
            ['test'],
            ['build'],
            ['lint'],
            ['check'],
        ):
            with self.subTest(recipe=recipe):
                result = subprocess.run(
                    [*command, '--dry-run', *recipe],
                    env=environment,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                arguments = shlex.split(result.stderr)
                self.assertEqual(arguments[:2], ['just', 'in-gallery'])
                override = f'CARGO_TARGET_DIR={root / ".tmp/gallery-target"}'
                self.assertEqual(override in arguments, recipe[0] in ('run', 'hot'))

    def test_gallery_adds_catalog_compiler_without_replacing_graphics_runtime(self) -> None:
        root = WRAPPER.parents[2]
        result = subprocess.run(
            [
                'just',
                '--justfile',
                str(root / 'infra/gallery/justfile'),
                '--dry-run',
                '--no-deps',
                'in-gallery',
                'cargo',
                'check',
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(
            shlex.split(result.stderr.replace('\\\n', '')),
            [
                str(WRAPPER),
                'nix',
                'shell',
                f'git+file://{root}#formatjs-cli',
                '--command',
                'env',
                f'CARGO_TARGET_DIR={root / ".tmp/gallery-check-target"}',
                '$@',
            ],
        )

    @unittest.skipUnless(sys.platform.startswith('linux'), 'Mold is selected only on Linux')
    def test_linker_preflight_rejects_incomplete_installation(self) -> None:
        for executable in (None, 'mold', 'ld.mold'):
            with self.subTest(executable=executable):
                result = self.linker_check(['require-linker'], executable)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('require mold and ld.mold on PATH', result.stderr)
                self.assertNotIn('cargo', result.stderr)
                self.assertEqual(result.stdout, '')

    @unittest.skipUnless(sys.platform.startswith('linux'), 'Mold is selected only on Linux')
    def test_linker_preflight_accepts_complete_installation(self) -> None:
        result = self.linker_check(['require-linker'], 'both')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, '')

    def linker_check(self, command: list[str], executable: str | None) -> subprocess.CompletedProcess[str]:
        just = shutil.which('just')
        bash = shutil.which('bash')
        assert just is not None and bash is not None
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'bash').symlink_to(bash)
            for name in ('mold', 'ld.mold'):
                if executable in (name, 'both'):
                    (root / name).symlink_to(bash)
            return subprocess.run(
                [just, '--tempdir', directory, '--justfile', str(WRAPPER.parents[1] / 'gallery/justfile'), *command],
                env={**os.environ, 'PATH': directory},
                capture_output=True,
                text=True,
                check=False,
            )

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
