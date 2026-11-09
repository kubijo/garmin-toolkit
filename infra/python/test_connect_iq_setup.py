import json
import stat
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import connect_iq
from connect_iq_manager import Runtime, extract_manager
from connect_iq_setup import rebuild, selected_sdk, setup


class WalkthroughTests(unittest.TestCase):
    def test_archive_cache_is_verified_and_cannot_escape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            archive = base / 'manager.zip'
            with zipfile.ZipFile(archive, 'w') as package:
                package.writestr('bin/sdkmanager', b'synthetic executable')
                package.writestr('share/sdkmanager/icon.png', b'icon')
            manager = extract_manager(archive, base / 'state')
            self.assertEqual(manager.read_bytes(), b'synthetic executable')
            self.assertEqual(manager.stat().st_mode & 0o777, 0o700)
            self.assertEqual(extract_manager(archive, base / 'state'), manager)
            manager.write_bytes(b'changed')
            with self.assertRaises(ValueError):
                extract_manager(archive, base / 'state')
            for name in ('../escaped', '/absolute', 'bin\\escaped'):
                with zipfile.ZipFile(archive, 'w') as package:
                    package.writestr('bin/sdkmanager', b'manager')
                    package.writestr(name, b'bad')
                with self.subTest(name=name), self.assertRaises(ValueError):
                    extract_manager(archive, base / 'unsafe')
            self.assertFalse((base / 'escaped').exists())

    def test_archive_rejects_symlinks_and_duplicate_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            archive = base / 'manager.zip'
            entry = zipfile.ZipInfo('bin/sdkmanager')
            entry.external_attr = (stat.S_IFLNK | 0o777) << 16
            with zipfile.ZipFile(archive, 'w') as package:
                package.writestr(entry, '../../elsewhere')
            with self.assertRaises(ValueError):
                extract_manager(archive, base / 'state')
            with zipfile.ZipFile(archive, 'w') as package:
                package.writestr('bin/sdkmanager', b'first')
                package.writestr('./bin/sdkmanager', b'second')
            with self.assertRaises(ValueError):
                extract_manager(archive, base / 'state')

    def test_active_sdk_configuration_is_platform_scoped_and_absolute(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            for system, folder in (
                ('Linux', '.Garmin/ConnectIQ'),
                ('Darwin', 'Library/Application Support/Garmin/ConnectIQ'),
            ):
                self.assertIsNone(selected_sdk(home, system))
                config = home / folder / 'current-sdk.cfg'
                config.parent.mkdir(parents=True)
                config.write_text(str(home / 'SDK with spaces') + '\n')
                self.assertEqual(selected_sdk(home, system), home / 'SDK with spaces')
                config.write_text('relative/path')
                with self.assertRaises(ValueError):
                    selected_sdk(home, system)

    def test_supplied_sdk_setup_does_not_launch_a_manager_and_rebuild_retains_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sdk = root / '.tmp/sdk'
            with (
                patch.object(connect_iq, 'ROOT', root),
                patch.object(connect_iq, 'keygen') as keygen,
                patch.object(connect_iq, 'doctor') as doctor,
                patch.object(connect_iq, 'build') as build,
                patch('connect_iq_setup.Runtime') as runtime,
            ):
                setup(sdk=sdk)
                runtime.assert_not_called()
                key = root / '.tmp/connect-iq/developer.der'
                keygen.assert_called_once_with(key=key)
                doctor.assert_not_called()  # The build owns its prerequisite check.
                self.assertEqual(build.call_args.kwargs['sdk'], sdk)
                self.assertEqual(build.call_args.kwargs['key'], key)
                settings = json.loads((root / '.tmp/connect-iq/local.json').read_text())
                self.assertEqual(settings, {'sdk': str(sdk), 'key': str(key)})
                rebuild(device='edge1050')
                self.assertEqual(build.call_count, 2)
                self.assertEqual(build.call_args.kwargs['device'], 'edge1050')
                first, second = (call.kwargs['output'] for call in build.call_args_list)
                self.assertNotEqual(first, second)

    def test_manager_image_never_contains_owner_material(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            context = root / 'infra/connect-iq'
            context.mkdir(parents=True)
            (context / 'Dockerfile').write_text('FROM fixture')
            (context / 'sources.list').write_text('fixture')
            certificate = root / 'ca-bundle.crt'
            certificate.write_text('fixture roots')
            with (
                patch('connect_iq_manager.platform.system', return_value='Linux'),
                patch('connect_iq_manager.platform.machine', return_value='x86_64'),
            ):
                runtime = Runtime(root, root / '.tmp/state', certificate=certificate)
            manager = root / '.tmp/download/bin/sdkmanager'
            command = runtime.command(manager, gui=False, environment={})
            self.assertIn('--cpus=2', command)
            self.assertIn('--memory=6g', command)
            self.assertIn(str(manager.parent.parent) + ':' + str(manager.parent.parent) + ':ro', command)
            self.assertEqual(command[-3:], ['/usr/bin/ldd', runtime.image, str(manager)])
            self.assertNotIn(str(Path.home()), command)
