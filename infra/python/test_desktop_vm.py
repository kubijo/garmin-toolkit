"""Check the desktop VM contract without building packages, booting guests, or capturing hardware."""

import subprocess
import tempfile
import unittest
from io import BytesIO
from pathlib import Path
from unittest.mock import Mock, patch

import desktop_vm as vm
from desktop_vm_qmp import Qmp
from json_data import object_rows, object_value, text


class DesktopVmTests(unittest.TestCase):
    def test_usb_selector_is_explicit_and_not_qemu_arguments(self) -> None:
        self.assertEqual(vm.usb_identity('091E:4c83'), (0x091E, 0x4C83))
        for invalid in ['', '091e', 'all', '091e:4c83,hostbus=1', '091e:4c83\n']:
            with self.subTest(value=invalid), self.assertRaises(ValueError):
                vm.usb_identity(invalid)

    def test_apple_silicon_uses_hvf_and_linux_arm_uses_kvm(self) -> None:
        for system, architecture, accelerator in [('Darwin', 'arm64', 'hvf'), ('Linux', 'aarch64', 'kvm')]:
            command = vm.machine_arguments(system, architecture, 'virt-10.2', Path('/firmware'))
            self.assertIn(accelerator, command)
            self.assertIn('host', command)
            self.assertIn('virt-10.2', command)
        command = vm.machine_arguments('Linux', 'x86_64', 'virt-10.2', Path('/firmware'))
        self.assertIn('tcg,thread=multi', command)
        with self.assertRaises(ValueError):
            vm.machine_arguments('Windows', 'aarch64', 'virt-10.2', Path('/firmware'))

    def test_guest_seed_pins_ubuntu_and_embeds_the_actual_guest_runner(self) -> None:
        config = vm.seed('ssh-ed25519 synthetic-test-key')
        user = object_rows(config['users'])[0]
        self.assertEqual(user['ssh_authorized_keys'], ['ssh-ed25519 synthetic-test-key'])
        sources = text(object_value(config['apt'])['sources_list'])
        self.assertNotIn('noble', sources)
        self.assertNotIn('ports.ubuntu.com', sources)
        for stanza in sources.strip().split('\n\n'):
            fields = dict(line.split(': ', 1) for line in stanza.splitlines())
            self.assertEqual(fields['Types'], 'deb')
            self.assertEqual(fields['Snapshot'], '20261017T120000Z')
            self.assertTrue(all(suite.startswith('resolute') for suite in fields['Suites'].split()))
        files = object_rows(config['write_files'])
        guest = next(file for file in files if file['path'] == '/opt/garmin-test/guest.py')
        self.assertEqual(guest['content'], (vm.SPEC / 'guest.py').read_text())
        self.assertFalse(config['package_upgrade'])

    def test_wrong_architecture_is_rejected_before_boot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / 'app'
            valid = b'\x7fELF\x02\x01\x01\x00AI\x02' + bytes(7) + (183).to_bytes(2, 'little')
            for invalid in [
                b'',
                b'\x7fELF',
                valid[:19],
                valid[:18] + (62).to_bytes(2, 'little'),
                valid[:8] + bytes(12),
            ]:
                artifact.write_bytes(invalid)
                with self.subTest(header=invalid), self.assertRaisesRegex(ValueError, 'ARM64'):
                    vm.build(artifact)
            artifact.write_bytes(valid)
            self.assertEqual(vm.build(artifact), artifact)

    def test_build_inherits_both_terminal_streams_and_resolves_its_output_link(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / 'app'
            artifact.write_bytes(b'\x7fELF\x02\x01\x01\x00AI\x02' + bytes(7) + (183).to_bytes(2, 'little'))
            result = subprocess.CompletedProcess(['nix'], 0, stdout=str(artifact) + '\n')
            link = Path(directory) / 'result'

            def produce(_command: list[str], **_options: object) -> subprocess.CompletedProcess[str]:
                link.symlink_to(artifact)
                return result

            with patch.object(vm.subprocess, 'run', side_effect=produce) as build:
                self.assertEqual(vm.build(None, out_link=link), artifact)
            arguments = build.call_args.args[0]
            self.assertIn('--print-build-logs', arguments)
            self.assertEqual(arguments[arguments.index('--out-link') + 1], str(link))
            self.assertNotIn('--log-format', arguments)
            self.assertNotIn('--print-out-paths', arguments)
            self.assertNotIn('stdout', build.call_args.kwargs)
            self.assertNotIn('stderr', build.call_args.kwargs)

    def test_vm_shutdown_runs_even_when_diagnostic_collection_is_interrupted(self) -> None:
        with patch.object(vm.subprocess, 'Popen') as start, patch.object(vm, 'stop_group') as stop:
            with self.assertRaises(KeyboardInterrupt), vm.running_vm(['qemu'], BytesIO()):
                raise KeyboardInterrupt
            stop.assert_called_once_with(start.return_value)
            self.assertTrue(start.call_args.kwargs['start_new_session'])

    def test_shutdown_failure_cannot_be_reported_as_success(self) -> None:
        with (
            patch.object(vm.subprocess, 'Popen'),
            patch.object(vm, 'stop_group', side_effect=OSError('still running')),
            self.assertRaisesRegex(OSError, 'still running'),
            vm.running_vm(['qemu'], BytesIO()),
        ):
            pass

    def test_user_owned_files_wait_until_cloud_init_has_created_the_user(self) -> None:
        config = vm.seed('ssh-ed25519 synthetic-test-key')
        for file in object_rows(config['write_files']):
            if file.get('owner') == 'tester:tester':
                self.assertIs(file.get('defer'), True)

    def test_user_namespace_permission_is_scoped_to_the_test_artifact(self) -> None:
        config = vm.seed('ssh-ed25519 synthetic-test-key')
        files = object_rows(config['write_files'])
        profile = next(file for file in files if file['path'] == '/etc/apparmor.d/garmin-desktop-test')
        self.assertIn('/home/tester/garmin-toolkit.AppImage', text(profile['content']))
        self.assertIn('userns,', text(profile['content']))
        self.assertNotIn('sysctl', str(config))

    def test_ambiguous_or_missing_host_usb_never_attaches_any_device(self) -> None:
        for inventory in ['', 'USB device 091e:4c83\nUSB device 091e:4c83']:
            with patch.object(Qmp, 'command', return_value=inventory) as command:
                client = object.__new__(Qmp)
                with self.assertRaisesRegex(ValueError, 'exactly one'):
                    client.attach(0x091E, 0x4C83)
                self.assertEqual(command.call_count, 1)

    def test_only_the_selected_usb_identity_is_attached(self) -> None:
        command = Mock(side_effect=['USB device 091e:4c83', {}])
        with patch.object(Qmp, 'command', command):
            object.__new__(Qmp).attach(0x091E, 0x4C83)
        self.assertEqual(
            command.call_args.args,
            (
                'device_add',
                {'driver': 'usb-host', 'id': 'garmin', 'vendorid': 0x091E, 'productid': 0x4C83},
            ),
        )

    def test_device_steps_cannot_be_skipped_or_confirmed_by_eof(self) -> None:
        self.assertEqual([name for name, _ in vm.CASE], ['discovery', 'activities', 'preview', 'dismissal'])
        process = Mock()
        process.poll.return_value = None
        with (
            patch.object(vm.select, 'select', return_value=([vm.sys.stdin], [], [])),
            patch.object(vm.sys.stdin, 'readline', return_value=''),
            self.assertRaisesRegex(ValueError, 'incomplete'),
        ):
            vm.confirm('Synthetic check', vm.time.monotonic() + 10, process)


if __name__ == '__main__':
    unittest.main()
