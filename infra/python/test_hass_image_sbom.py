"""Exercise the real scanner with project-owned synthetic ELF, WASM, and Nix layers."""

import gzip
import hashlib
import json
import os
import pty
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zlib
from copy import deepcopy
from pathlib import Path
from unittest.mock import patch

from archive_io import expand
from fixtures.images import tar_bytes, write_image
from haos_platform import ImageArchive, inspect_archive
from hass_image_inventory import image_path, inventory_image, wasm_dependencies
from hass_image_sbom import generate, verify_sbom
from hass_release_build import NativeBuild, native_derivation, record_build


def leb(value: int) -> bytes:
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def audit(name: str) -> bytes:
    return zlib.compress(
        json.dumps(
            {
                'packages': [
                    {
                        'name': name,
                        'version': '0.1.0',
                        'source': 'local',
                        'root': True,
                        'dependencies': [1],
                    },
                    {
                        'name': 'synthetic-dependency',
                        'version': '1.2.3',
                        'source': 'crates.io',
                        'dependencies': [],
                    },
                ]
            }
        ).encode()
    )


def wasm(name: str = 'garmin-hass-web') -> bytes:
    section = b'\x07.dep-v0' + audit(name)
    return b'\0asm\x01\0\0\0\0' + leb(len(section)) + section


def elf(name: str) -> bytes:
    strings = b'\0.shstrtab\0.dep-v0\0'
    data = audit(name)
    offset = 64 + 3 * 64
    header = b'\x7fELF\x02\x01\x01' + b'\0' * 9
    header += struct.pack('<HHIQQQIHHHHHH', 2, 62, 1, 0, 0, 64, 0, 64, 0, 0, 64, 3, 1)
    sections = b'\0' * 64
    sections += struct.pack('<IIQQQQIIQQ', 1, 3, 0, 0, offset, len(strings), 0, 0, 1, 0)
    sections += struct.pack('<IIQQQQIIQQ', 11, 1, 0, 0, offset + len(strings), len(data), 0, 0, 1, 0)
    return header + sections + strings + data


class ImageSbomTests(unittest.TestCase):
    def test_nix_derivation_versions_preserve_native_system_and_exact_output(self) -> None:
        path = Path('/nix/store/' + '0' * 32 + '-image.drv')
        output = '/nix/store/' + '1' * 32 + '-image.tar.gz'
        modern = {
            'version': 4,
            'derivations': {
                path.name: {
                    'system': 'x86_64-linux',
                    'outputs': {'out': {'path': Path(output).name}},
                }
            },
        }
        legacy = {str(path): {'system': 'x86_64-linux', 'outputs': {'out': {'path': output}}}}
        for document in (modern, legacy):
            result = subprocess.CompletedProcess(['nix'], 0, stdout=json.dumps(document))
            with patch('hass_release_build.subprocess.run', return_value=result):
                self.assertEqual(native_derivation(path), ('x86_64-linux', Path(output)))

    def setUp(self) -> None:
        temporary = Path(__file__).resolve().parents[2] / '.tmp'
        temporary.mkdir(exist_ok=True)
        self.scratch = tempfile.TemporaryDirectory(prefix='image-sbom-tests.', dir=temporary)
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.store = '/nix/store/' + '0' * 32 + '-garmin-hass-0.1.0'
        self.native = {
            self.store.lstrip('/') + '/bin/garmin-hass': elf('garmin-hass'),
            self.store.lstrip('/') + '/bin/garmin-gpx-worker': elf('garmin-gpx-worker'),
            self.store.lstrip('/') + '/share/garmin-hass/web/app.wasm': wasm(),
            self.store.lstrip('/') + '/share/garmin-hass/web/index.html': b'<html>synthetic</html>',
        }
        self.archive = self.root / 'image.tar'
        write_image(self.archive, files=self.native)
        self.closure = self.root / 'store-paths'
        self.closure.write_text(self.store + '\n')
        self.bundle = self.root / 'bundle.json'
        self.bundle.write_text(
            json.dumps(
                {
                    'entries': [
                        {
                            'name': 'synthetic-font',
                            'version': '1',
                            'license': 'MIT',
                            'source': {'kind': 'asset', 'url': 'https://example.invalid/synthetic-font'},
                            'notices': [{'text_index': 0}],
                        }
                    ],
                    'license_texts': ['Synthetic project-owned test notice.'],
                }
            )
        )
        self.output = self.root / 'image.spdx.json'

    def test_wasm_requires_embedded_runtime_dependencies(self) -> None:
        self.assertEqual(
            [entry['name'] for entry in wasm_dependencies(wasm())], ['garmin-hass-web', 'synthetic-dependency']
        )
        for invalid in (b'', b'\0asm\x01\0\0\0', wasm('another-app'), wasm()[:-1]):
            with self.subTest(invalid=invalid[:12]), self.assertRaises(ValueError):
                wasm_dependencies(invalid)

    def test_image_inventory_hashes_actual_layer_bytes(self) -> None:
        self.native[self.store.lstrip('/') + '/bin/garmin-hass'] += b'\0' * (2 * 1024 * 1024)
        write_image(self.archive, files=self.native)
        inventory = inventory_image(self.archive, inspect_archive(self.archive))
        self.assertEqual(inventory.store_paths, {self.store})
        self.assertEqual(
            inventory.hashes, {'/' + name: hashlib.sha256(value).hexdigest() for name, value in self.native.items()}
        )

    def test_conflicting_layer_files_cannot_reuse_stale_dependency_metadata(self) -> None:
        layer = tar_bytes(self.native)
        self.archive.write_bytes(tar_bytes({'one.tar': layer, 'two.tar': layer}))
        inspected = ImageArchive({'architecture': 'amd64'}, '', (), ('one.tar', 'two.tar'))
        with self.assertRaisesRegex(ValueError, 'Duplicate image file'):
            inventory_image(self.archive, inspected)

    def test_real_scanner_covers_native_wasm_assets_and_every_file(self) -> None:
        self.assertIsNotNone(shutil.which('syft'), 'The pinned Syft scanner is required')
        write_image(
            self.archive, files={'/': None, '/nix': None, **{'/' + name: data for name, data in self.native.items()}}
        )
        generate(self.archive, self.closure, self.bundle, self.output, 1)
        bundle = json.loads(self.bundle.read_bytes())
        verify_sbom(self.archive, self.output, bundle)
        document = json.loads(self.output.read_bytes())
        self.assertIn('/', {entry['fileName'] for entry in document['files']})
        names = {entry['name'] for entry in document['packages']}
        self.assertTrue({'garmin-hass', 'garmin-gpx-worker', 'garmin-hass-web', 'synthetic-font'}.issubset(names))
        packages, edges = document['packages'], document['relationships']

        def index(prefix: str) -> int:
            return next(i for i, row in enumerate(packages) if row['SPDXID'].startswith(prefix))

        wasm_index, native_index = index('SPDXRef-WasmCrate-'), index('SPDXRef-Package-rust-crate-')
        cases: list[tuple[str, tuple[str | int, ...], object]] = [
            ('hash', ('packages', index('SPDXRef-GarminImage'), 'checksums'), []),
            ('file', ('files',), []),
            ('empty-file-name', ('files', 0, 'fileName'), ''),
            ('name', ('packages', wasm_index, 'name'), 'forged'),
            ('version', ('packages', wasm_index, 'versionInfo'), 'forged'),
            ('purl', ('packages', wasm_index, 'externalRefs'), []),
            ('native-purl', ('packages', native_index, 'externalRefs'), []),
            ('relationships', ('relationships',), []),
            ('native-relationship', ('relationships',), [row for row in edges if row['relationshipType'] != 'OTHER']),
            ('dangling', ('relationships', 0, 'relatedSpdxElement'), 'SPDXRef-Missing'),
            ('duplicate-package', ('packages',), [*packages, packages[wasm_index]]),
            ('duplicate-file', ('files',), [*document['files'], document['files'][0]]),
            ('duplicate-global', ('packages', wasm_index, 'SPDXID'), document['files'][0]['SPDXID']),
            ('packages', ('packages',), [None]),
            ('files', ('files',), [None]),
            ('root', (), []),
            ('license', ('hasExtractedLicensingInfos',), []),
            ('license-text', ('hasExtractedLicensingInfos', 0, 'extractedText'), 'forged'),
        ]
        for prefix in ('NixOutput', 'WasmCrate', 'Asset', 'Package-rust-crate'):
            cases.append(
                (prefix, ('packages',), [row for row in packages if not row['SPDXID'].startswith('SPDXRef-' + prefix)])
            )
        for change, path, value in cases:
            modified = deepcopy(document)
            parent = modified
            for segment in path[:-1]:
                parent = parent[segment]
            if path:
                parent[path[-1]] = value
            else:
                modified = value
            tampered = self.root / 'tampered.json'
            tampered.write_text(json.dumps(modified))
            with self.subTest(change=change), self.assertRaises(ValueError):
                verify_sbom(self.archive, tampered, bundle)

    def test_layer_paths_accept_container_root_entries_and_reject_traversal(self) -> None:
        for name in ('nix/store', './nix/store', '/nix/store', '//nix/store'):
            self.assertEqual(image_path(name), '/nix/store')
        for name in ('../nix', '/nix/../escape', 'nix/../../escape'):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'Unsafe image member'):
                image_path(name)

    def test_real_scanner_handles_nix_terminal_output_without_interactive_input(self) -> None:
        master, terminal = pty.openpty()
        try:
            subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name('hass_image_sbom.py')),
                    '--archive',
                    str(self.archive),
                    '--closure',
                    str(self.closure),
                    '--bundle',
                    str(self.bundle),
                    '--output',
                    str(self.output),
                    '--created',
                    '1',
                ],
                env=os.environ | {'NO_TTY': 'false'},
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=terminal,
                start_new_session=True,
                check=True,
                timeout=30,
            )
        finally:
            os.close(terminal)
            os.close(master)
        verify_sbom(self.archive, self.output, json.loads(self.bundle.read_bytes()))

    def test_wrong_runtime_closure_does_not_create_output(self) -> None:
        self.closure.write_text(self.store + '-other\n')
        with self.assertRaisesRegex(ValueError, 'runtime closure'):
            generate(self.archive, self.closure, self.bundle, self.output, 1)
        self.assertFalse(self.output.exists())

    def test_archive_limits_reject_small_bombs_before_scanning(self) -> None:
        for limit, value in (('MAX_IMAGE_BYTES', 1024), ('MAX_MEMBERS', 1)):
            with self.subTest(limit=limit), patch('archive_io.' + limit, value):
                with patch('hass_image_sbom.subprocess.run') as scanner, self.assertRaises(ValueError):
                    generate(self.archive, self.closure, self.bundle, self.output, 1)
                scanner.assert_not_called()
                self.assertFalse(self.output.exists())
        compressed = self.root / 'bomb.gz'
        compressed.write_bytes(gzip.compress(b'x' * 8192))
        expanded = self.root / 'expanded.tar'
        with patch('archive_io.MAX_IMAGE_BYTES', 1024), self.assertRaises(ValueError):
            expand(compressed, expanded)
        self.assertLessEqual(expanded.stat().st_size, 1024)
        trailing = self.root / 'trailing.tar.gz'
        trailing.write_bytes(gzip.compress(self.archive.read_bytes() + b'x' * 200_000))
        with patch('archive_io.MAX_IMAGE_BYTES', 100_000), self.assertRaisesRegex(ValueError, 'size limit'):
            inspect_archive(trailing)
        header = tarfile.TarInfo('long-name')
        header.type, header.size = tarfile.GNUTYPE_LONGNAME, 2 * 1024 * 1024
        self.archive.write_bytes(header.tobuf() + b'x' * header.size + b'\0' * 1024)
        with self.assertRaisesRegex(ValueError, 'extension metadata'):
            inspect_archive(self.archive)

    def native_build(self) -> NativeBuild:
        source = self.root / 'source'
        source.mkdir()
        (source / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0"\n')
        metadata = source / 'apps/garmin-hass/addon/config.yaml'
        metadata.parent.mkdir(parents=True)
        metadata.write_text('version: 0.1.0\nimage: ghcr.io/kubijo/garmin-toolkit-hass\n')
        (source / 'flake.lock').write_text('{}\n')
        bundle = source / 'assets/licenses/bundle-hass.json'
        bundle.parent.mkdir(parents=True)
        shutil.copyfile(self.bundle, bundle)
        environment = {
            **os.environ,
            'GIT_CONFIG_NOSYSTEM': '1',
            'GIT_CONFIG_GLOBAL': os.devnull,
            'GIT_AUTHOR_NAME': 'Synthetic',
            'GIT_AUTHOR_EMAIL': 'synthetic@example.invalid',
            'GIT_COMMITTER_NAME': 'Synthetic',
            'GIT_COMMITTER_EMAIL': 'synthetic@example.invalid',
        }
        for args in (['init', '-b', 'main'], ['add', '.'], ['commit', '-m', 'Synthetic build source']):
            subprocess.run(['git', '-C', str(source), *args], env=environment, check=True, capture_output=True)
        return NativeBuild(
            source=source,
            archive=self.archive,
            sbom=self.output,
            derivation=Path('/nix/store/' + '0' * 32 + '-synthetic-image.drv'),
            output=self.root / 'recorded',
        )

    def test_native_receipt_binds_real_scanned_files_and_rejects_wrong_derivation(self) -> None:
        generate(self.archive, self.closure, self.bundle, self.output, 1)
        options = self.native_build()

        with (
            patch('hass_release_build.platform.machine', return_value='x86_64'),
            patch('hass_release_build.platform.system', return_value='Linux'),
            patch('hass_release_build.native_derivation', return_value=('x86_64-linux', self.archive)) as producing,
        ):
            record_build(options)
            receipt = json.loads((options.output / 'native-build.json').read_bytes())
            self.assertIsNone(receipt['source']['tag'])
            self.assertIsNone(receipt['platform']['workflow_run'])
            self.assertNotIn('manifest_digest', receipt['platform'])
            self.assertEqual(
                receipt['platform']['archive_sha256'], hashlib.sha256(self.archive.read_bytes()).hexdigest()
            )
            self.assertEqual(len(list(options.output.iterdir())), 3)
            shutil.rmtree(options.output)
            producing.return_value = ('aarch64-linux', self.archive)
            with self.assertRaisesRegex(ValueError, 'native builder'):
                record_build(options)
            self.assertFalse(options.output.exists())

    def test_generated_inventory_is_stable_and_scanner_failure_cleans_scratch(self) -> None:
        compressed = self.root / 'image.tar.gz'
        compressed.write_bytes(gzip.compress(self.archive.read_bytes(), mtime=0))
        self.archive = compressed
        generate(self.archive, self.closure, self.bundle, self.output, 1)
        again = self.root / 'again.spdx.json'
        generate(self.archive, self.closure, self.bundle, again, 1)
        self.assertEqual(self.output.read_bytes(), again.read_bytes())
        failed = self.root / 'failed.spdx.json'
        temporary = self.root / 'scratch'
        temporary.mkdir()
        with (
            patch.dict(os.environ, {'NIX_BUILD_TOP': str(temporary)}),
            patch('hass_image_sbom.subprocess.run', side_effect=subprocess.CalledProcessError(17, ['syft'])),
            self.assertRaises(subprocess.CalledProcessError),
        ):
            generate(self.archive, self.closure, self.bundle, failed, 1)
        self.assertFalse(failed.exists())
        self.assertEqual(list(temporary.iterdir()), [])


if __name__ == '__main__':
    os.environ.setdefault('SYFT_CACHE_DIR', '')
    unittest.main()
