import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

LAUNCHER = Path(__file__).resolve().parents[1] / 'nix/host-graphics/launch.sh'


@unittest.skipUnless(sys.platform.startswith('linux'), 'The AppImage launcher targets Linux')
class HostGraphicsTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        bash = shutil.which('bash')
        assert bash is not None
        self.bash: str = bash
        self.bundled = self.directory('bundled')
        self.host = self.directory('host multiarch')
        self.inherited = self.directory('inherited')
        self.manifest = self.root / 'libraries'
        self.manifest.write_text(f'{self.bundled}\n')
        self.cache = self.root / 'ld.so.cache'
        self.cache.write_text('fixture')
        self.reader = self.script(
            'cache-reader',
            'printf "%s\\n" "$@" > "$READER_ARGUMENTS"\n'
            'printf "%s\\n" "$LD_LIBRARY_PATH" > "$READER_LIBRARIES"\n'
            'printf "%s\\n" "$CACHE_CONTENTS"\n',
        )
        self.application = self.script(
            'application',
            'printf "%s\\n" "$LD_LIBRARY_PATH"\n'
            'printf "%s\\n" "$$"\n'
            'printf "%s\\n" "$@"\n'
            'exit "${APPLICATION_STATUS:-0}"\n',
        )
        self.environment: dict[str, str] = {
            **os.environ,
            'LD_LIBRARY_PATH': f':relative::{self.inherited}:{self.bundled}:',
            'READER_ARGUMENTS': str(self.root / 'reader-arguments'),
            'READER_LIBRARIES': str(self.root / 'reader-libraries'),
            'CACHE_CONTENTS': (
                f'3 libs found in cache\n'
                f'\tlibdriver.so (libc6) => {self.host}/libdriver.so\n'
                f'\tlibhelper.so (libc6) => {self.host}/libhelper.so\n'
                '\tlibmissing.so (libc6) => /does-not-exist/libmissing.so\n'
            ),
        }
        for variable in ('GIO_EXTRA_MODULES', 'XDG_DATA_DIRS'):
            self.environment.pop(variable, None)

    def directory(self, name: str) -> Path:
        directory = self.root / name
        directory.mkdir()
        return directory

    def script(self, name: str, body: str) -> Path:
        script = self.root / name
        script.write_text(f'#!{self.bash}\nset -eu\n{body}')
        script.chmod(0o755)
        return script

    def launch(self, *arguments: str) -> tuple[int, str, str, int]:
        with subprocess.Popen(
            [
                self.bash,
                str(LAUNCHER),
                str(self.manifest),
                str(self.reader),
                str(self.cache),
                str(self.application),
                *arguments,
            ],
            env=self.environment,
            cwd=self.root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        ) as process:
            output, errors = process.communicate(timeout=10)
            return process.returncode, output, errors, process.pid

    def test_bundle_precedes_host_and_arguments_exit_status_and_pid_survive(self) -> None:
        arguments = ['--control-server', 'spaces and * globs', '', '$(must-not-run)']
        self.environment['APPLICATION_STATUS'] = '37'
        status, output, errors, pid = self.launch(*arguments)
        self.assertEqual(status, 37, errors)
        self.assertEqual(errors, '')
        libraries, application_pid, *actual_arguments = output.splitlines()
        directories = libraries.split(':')
        self.assertEqual(directories[:3], [str(self.bundled), str(self.inherited), str(self.host)])
        self.assertEqual(len(directories), len(set(directories)))
        self.assertTrue(all(Path(directory).is_absolute() for directory in directories))
        self.assertNotIn('/does-not-exist', directories)
        self.assertEqual(actual_arguments, arguments)
        self.assertEqual(int(application_pid), pid)
        self.assertEqual((self.root / 'reader-arguments').read_text().splitlines(), ['-p', '-C', str(self.cache)])
        self.assertEqual((self.root / 'reader-libraries').read_text().strip(), f'{self.bundled}:{self.inherited}')

    def test_missing_cache_still_launches_and_never_calls_cache_reader(self) -> None:
        self.cache.unlink()
        status, output, errors, _ = self.launch()
        self.assertEqual(status, 0, errors)
        self.assertEqual(errors, '')
        self.assertEqual(output.splitlines()[0].split(':')[:2], [str(self.bundled), str(self.inherited)])
        self.assertFalse((self.root / 'reader-arguments').exists())

    def test_launch_needs_no_inherited_library_path(self) -> None:
        del self.environment['LD_LIBRARY_PATH']
        status, output, errors, _ = self.launch()
        self.assertEqual(status, 0, errors)
        directories = output.splitlines()[0].split(':')
        self.assertEqual(directories[:2], [str(self.bundled), str(self.host)])
        self.assertNotIn('', directories)

    def test_failed_cache_read_warns_but_preserves_bundled_libraries(self) -> None:
        self.reader = self.script('failing-reader', 'exit 1\n')
        status, output, errors, _ = self.launch()
        self.assertEqual(status, 0, errors)
        self.assertIn('could not read', errors)
        self.assertEqual(output.splitlines()[0].split(':')[:2], [str(self.bundled), str(self.inherited)])

    def test_unavailable_bundle_fails_before_reading_cache_or_launching(self) -> None:
        self.manifest.write_text('/does-not-exist\n')
        status, output, errors, _ = self.launch()
        self.assertNotEqual(status, 0)
        self.assertEqual(output, '')
        self.assertIn('bundled library directories are unavailable', errors)
        self.assertFalse((self.root / 'reader-arguments').exists())

    def test_host_directory_names_are_never_evaluated_as_shell_code(self) -> None:
        unexpected = self.root / 'unexpected'
        host = self.directory('$(touch unexpected) [driver]')
        self.environment['CACHE_CONTENTS'] = f'libdriver.so (libc6) => {host}/libdriver.so'
        status, output, errors, _ = self.launch()
        self.assertEqual(status, 0, errors)
        self.assertIn(str(host), output.splitlines()[0].split(':'))
        self.assertFalse(unexpected.exists())

    def test_host_snapshot_distinguishes_unset_empty_and_custom_values(self) -> None:
        self.environment.pop('LD_LIBRARY_PATH')
        self.environment['GIO_EXTRA_MODULES'] = ''
        self.environment['XDG_DATA_DIRS'] = '/host/share with spaces'
        self.application = self.script(
            'snapshot',
            'for variable in LD_LIBRARY_PATH GIO_EXTRA_MODULES XDG_DATA_DIRS; do\n'
            '  saved="NIX_APPIMAGE_HOST_$variable"\n'
            '  marker="${saved}_SET"\n'
            '  printf "%s=%s:%s\\n" "$variable" "${!marker}" "${!saved}"\n'
            'done\n',
        )
        status, output, errors, _ = self.launch()
        self.assertEqual(status, 0, errors)
        self.assertEqual(
            output.splitlines(),
            ['LD_LIBRARY_PATH=:', 'GIO_EXTRA_MODULES=x:', 'XDG_DATA_DIRS=x:/host/share with spaces'],
        )


if __name__ == '__main__':
    unittest.main()
