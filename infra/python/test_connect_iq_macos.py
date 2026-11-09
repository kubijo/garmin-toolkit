import plistlib
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from connect_iq_macos import install_manager


class MacInstallTests(unittest.TestCase):
    def test_native_mount_copy_and_detach_preserve_the_app_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            image = root / 'Manager.dmg'
            image.write_bytes(b'owner-downloaded fixture')
            commands: list[list[str]] = []

            def run(command: list[str], **_options: object) -> subprocess.CompletedProcess[bytes]:
                commands.append(command)
                if command[1] == 'attach':
                    mount = Path(command[command.index('-mountpoint') + 1])
                    app = mount / 'SDKManager.app'
                    app.mkdir()
                    (app / 'fixture').write_bytes(b'app resources')
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        plistlib.dumps(
                            {
                                'system-entities': [
                                    {'dev-entry': '/dev/fake'},
                                    {'mount-point': str(mount)},
                                ]
                            }
                        ),
                    )
                if command[0] == '/usr/bin/ditto':
                    shutil.copytree(command[1], command[2])
                elif command[1] == 'detach':
                    shutil.rmtree(Path(command[2]) / 'SDKManager.app')
                return subprocess.CompletedProcess(command, 0, b'')

            with patch('connect_iq_macos.subprocess.run', side_effect=run):
                app = install_manager(image, root / 'state')
            self.assertEqual((app / 'fixture').read_bytes(), b'app resources')
            self.assertIn('-readonly', commands[0])
            self.assertIn('-nobrowse', commands[0])
            self.assertEqual(commands[-1][1], 'detach')
            self.assertEqual(list((root / 'state').glob('dmg-*')), [])

    def test_failed_copy_still_detaches_the_owned_mount(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            image = root / 'Manager.dmg'
            image.write_bytes(b'fixture')
            detached: list[Path] = []

            def run(command: list[str], **_options: object) -> subprocess.CompletedProcess[bytes]:
                if command[1] == 'attach':
                    mount = Path(command[command.index('-mountpoint') + 1])
                    (mount / 'SDKManager.app').mkdir()
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        plistlib.dumps(
                            {
                                'system-entities': [{'mount-point': str(mount)}],
                            }
                        ),
                    )
                if command[0] == '/usr/bin/ditto':
                    partial = Path(command[2])
                    partial.mkdir()
                    (partial / 'incomplete').write_bytes(b'partial copy')
                    raise subprocess.CalledProcessError(7, command)
                mount = Path(command[2])
                shutil.rmtree(mount / 'SDKManager.app')
                detached.append(mount)
                return subprocess.CompletedProcess(command, 0, b'')

            with (
                patch('connect_iq_macos.subprocess.run', side_effect=run),
                self.assertRaises(subprocess.CalledProcessError),
            ):
                install_manager(image, root / 'state')
            self.assertEqual(len(detached), 1)
            self.assertFalse(detached[0].exists())
            self.assertEqual(list((root / 'state/manager').rglob('*.app')), [])
