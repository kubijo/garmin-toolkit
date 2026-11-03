"""Exercise deployment against synthetic Supervisor CLI responses without SSH."""

import io
import json
import subprocess
import unittest
from pathlib import Path

from cli_output import TerminalOutput
from hass_deploy_remote import DEV_SLUG, Installer


class FakeTransport:
    def __init__(
        self,
        *,
        installed: bool = False,
        reload_timeout: bool = False,
        fail_start: bool = False,
        fail_install: bool = False,
    ) -> None:
        self.installed = installed
        self.reload_timeout = reload_timeout
        self.fail_start = fail_start
        self.fail_install = fail_install
        self.commands: list[tuple[str, ...]] = []
        self.dev_state = 'started' if installed else 'unknown'

    def upload(self, archive: Path, destination: str) -> None:
        self.commands.append(('upload', str(archive), destination))

    def execute(self, *arguments: str, timeout: int = 120) -> str:
        self.commands.append(arguments)
        if arguments[0] == 'mktemp':
            return '/tmp/garmin-toolkit-dev.Abc123\n'
        if arguments[0] != 'ha':
            return ''
        command = arguments[1:-1]
        if command == ('store', 'reload') and self.reload_timeout:
            raise subprocess.CalledProcessError(1, arguments, stderr='context deadline exceeded')
        if command[:2] == ('apps', 'info'):
            stable = command[2] == 'stable_test'
            data = {
                'slug': command[2],
                'state': 'started' if stable else self.dev_state,
                'version': '0.1.5' if stable else '0.1.5-dev' if self.installed else None,
                'version_latest': '0.1.5-dev',
                'available': True,
            }
            # Installed app details omit the Store-only `installed` flag.
            return json.dumps({'result': 'ok', 'data': data})
        if command[:2] == ('apps', 'install'):
            if self.installed or self.fail_install:
                return json.dumps({'result': 'error', 'message': 'App installation failed'})
            self.installed = True
            self.dev_state = 'stopped'
        if command[:2] == ('apps', 'start') and command[2] == DEV_SLUG:
            if self.fail_start:
                return json.dumps({'result': 'error', 'message': 'App failed to start'})
            self.dev_state = 'started'
        return json.dumps({'result': 'ok', 'data': {}})


class RemoteDeployTests(unittest.TestCase):
    def install(self, transport: FakeTransport) -> None:
        stream = io.StringIO()
        output = TerminalOutput(stdout=stream, stderr=stream)
        Installer(transport, output).install(Path('bundle.tar.gz'), '/local_apps', 'stable_test', '0.1.5-dev')

    def test_store_entry_with_null_version_is_installed_before_stable_stops(self) -> None:
        transport = FakeTransport()
        self.install(transport)
        install = ('ha', 'apps', 'install', DEV_SLUG, '--raw-json')
        stop = ('ha', 'apps', 'stop', 'stable_test', '--raw-json')
        self.assertLess(transport.commands.index(install), transport.commands.index(stop))

    def test_installed_app_without_installed_flag_is_rebuilt(self) -> None:
        transport = FakeTransport(installed=True)
        self.install(transport)
        rebuild = ('ha', 'apps', 'rebuild', DEV_SLUG, '--raw-json')
        self.assertIn(rebuild, transport.commands)
        self.assertNotIn(('ha', 'apps', 'install', DEV_SLUG, '--raw-json'), transport.commands)
        self.assertLess(
            transport.commands.index(('ha', 'apps', 'stop', DEV_SLUG, '--raw-json')),
            transport.commands.index(rebuild),
        )

    def test_reload_timeout_continues_when_expected_version_is_available(self) -> None:
        transport = FakeTransport(reload_timeout=True)
        self.install(transport)
        self.assertIn(('ha', 'apps', 'install', DEV_SLUG, '--raw-json'), transport.commands)

    def test_failed_start_stops_dev_and_restores_stable(self) -> None:
        transport = FakeTransport(fail_start=True)
        with self.assertRaisesRegex(ValueError, 'failed to start'):
            self.install(transport)
        self.assertEqual(
            transport.commands[-3:-1],
            [('ha', 'apps', 'stop', DEV_SLUG, '--raw-json'), ('ha', 'apps', 'start', 'stable_test', '--raw-json')],
        )

    def test_supervisor_error_with_zero_process_exit_keeps_stable_running(self) -> None:
        transport = FakeTransport(fail_install=True)
        with self.assertRaisesRegex(ValueError, 'installation failed'):
            self.install(transport)
        self.assertNotIn(('ha', 'apps', 'stop', 'stable_test', '--raw-json'), transport.commands)
        self.assertEqual(transport.commands[-1], ('rm', '-f', '--', '/tmp/garmin-toolkit-dev.Abc123'))


if __name__ == '__main__':
    unittest.main()
