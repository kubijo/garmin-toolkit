import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import connect_iq


class LocalToolTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('openssl'), 'OpenSSL is required for key generation')
    def test_generated_key_is_compatible_der_and_private(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            key = Path(temporary) / 'developer.der'
            connect_iq.keygen(key=key)
            subprocess.run(
                ['openssl', 'pkcs8', '-inform', 'DER', '-in', str(key), '-nocrypt', '-out', '/dev/null'],
                check=True,
                capture_output=True,
            )
            connect_iq.validate_key(connect_iq.private_key(key))
            self.assertEqual(key.stat().st_mode & 0o777, 0o600)
            original = key.read_bytes()
            connect_iq.keygen(key=key)
            self.assertEqual(key.read_bytes(), original)

    @unittest.skipUnless(shutil.which('openssl'), 'OpenSSL is required for key conversion')
    def test_legacy_der_conversion_preserves_identity_and_original_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original, legacy, converted = root / 'original.der', root / 'legacy.der', root / 'converted.der'
            connect_iq.keygen(key=original)
            subprocess.run(
                [
                    'openssl',
                    'rsa',
                    '-inform',
                    'DER',
                    '-in',
                    str(original),
                    '-traditional',
                    '-outform',
                    'DER',
                    '-out',
                    str(legacy),
                ],
                check=True,
                capture_output=True,
            )
            legacy.chmod(0o600)
            retained = legacy.read_bytes()
            with self.assertRaises(subprocess.CalledProcessError):
                connect_iq.validate_key(legacy)
            connect_iq.keygen(key=converted, from_key=legacy)
            with self.assertRaises(ValueError):
                connect_iq.keygen(key=converted, from_key=legacy)
            self.assertEqual(legacy.read_bytes(), retained)
            public = [
                subprocess.run(
                    ['openssl', 'pkey', '-inform', 'DER', '-in', str(path), '-pubout', '-outform', 'DER'],
                    check=True,
                    capture_output=True,
                ).stdout
                for path in (legacy, converted)
            ]
            self.assertEqual(public[0], public[1])

    def test_material_cannot_enter_tracked_source_or_escape_through_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / 'repo'
            root.mkdir()
            (root / '.tmp').mkdir()
            self.assertEqual(connect_iq.local_material(root / '.tmp/key', root), root / '.tmp/key')
            with self.assertRaises(ValueError):
                connect_iq.local_material(root / 'key', root)
            (base / 'alias').symlink_to(root, target_is_directory=True)
            with self.assertRaises(ValueError):
                connect_iq.local_material(base / 'alias/key', root)

    def test_key_must_be_private(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            key = base / 'key'
            key.write_bytes(b'test key')
            key.chmod(0o600)
            self.assertEqual(connect_iq.private_key(key, base / 'repo'), key)
            key.chmod(0o644)
            with self.assertRaises(ValueError):
                connect_iq.private_key(key, base / 'repo')

    def test_build_is_explicit_local_and_never_overwrites_output(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / 'repo'
            root.mkdir()
            key = base / 'key'
            key.write_bytes(b'test key')
            key.chmod(0o600)
            sdk = base / 'sdk'
            output = root / '.tmp/build'
            with (
                patch.object(connect_iq, 'ROOT', root),
                patch.object(connect_iq, 'doctor') as doctor,
                patch.object(connect_iq.subprocess, 'run') as run,
            ):

                def compile_probe(command: list[str], **_options: object) -> None:
                    Path(command[command.index('-o') + 1]).write_bytes(b'compiled fixture')

                run.side_effect = compile_probe
                connect_iq.build(sdk=sdk, key=key, output=output)
                self.assertEqual((root / 'dist/connect-iq/probe.prg').read_bytes(), b'compiled fixture')
                doctor.assert_called_once_with(sdk=sdk, key=key)
                arguments = run.call_args.args[0]
                self.assertEqual(arguments[0], str(sdk / 'bin/monkeyc'))
                self.assertIn(str(key), arguments)
                self.assertIn(str(output / 'probe.prg'), arguments)
                self.assertIn('fenix8solar47mm', arguments)
                with self.assertRaises(FileExistsError):
                    connect_iq.build(sdk=sdk, key=key, output=output)
                with self.assertRaises(ValueError):
                    connect_iq.build(sdk=sdk, key=key, output=root / 'release')
                self.assertEqual(run.call_count, 1)
                connect_iq.build(sdk=sdk, key=key, output=root / '.tmp/edge', device='edge1050')
                self.assertIn('edge1050', run.call_args.args[0])
                self.assertEqual((root / 'dist/connect-iq/edge1050/probe.prg').read_bytes(), b'compiled fixture')
                self.assertEqual((root / 'dist/connect-iq/probe.prg').read_bytes(), b'compiled fixture')

    def test_keygen_does_not_replace_existing_material(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            key = Path(temporary) / 'key'
            key.write_bytes(b'owner material')
            key.chmod(0o600)
            with (
                patch.object(connect_iq, 'validate_key') as validate,
                patch.object(connect_iq.subprocess, 'run') as run,
            ):
                connect_iq.keygen(key=key)
                validate.assert_called_once_with(key)
                run.assert_not_called()
                self.assertEqual(key.read_bytes(), b'owner material')
