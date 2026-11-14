"""Validate real SquashFS inventories and source coverage without Nix builds or foreign execution."""

import hashlib
import subprocess
import sys
import tempfile
import tracemalloc
import unittest
from pathlib import Path
from unittest.mock import patch

import desktop_sources as sources
from desktop_source_fixtures import STATIC_LIBRARY, STORE_PATH, source_assets
from json_data import file_sha256, json_bytes, object_rows, object_value, read_json

TAG, COMMIT = 'v0.1.5', '1' * 40
PREFIX, APP_RUN = b'synthetic runtime\n', b'synthetic AppRun\n'


class DesktopSourcesTests(unittest.TestCase):
    def test_oversized_app_run_is_rejected_without_buffering_the_decompressed_file(self) -> None:
        tree = self.root / 'tree'
        with (tree / 'AppRun').open('wb') as stream:
            stream.truncate(64 * 1024**2)
        image = self.root / 'oversized.squashfs'
        subprocess.run(
            ['mksquashfs', str(tree), str(image), '-noappend', '-processors', '1', '-no-progress', '-no-xattrs'],
            check=True,
            capture_output=True,
        )
        tracemalloc.start()
        try:
            with self.assertRaisesRegex(ValueError, 'size limit'):
                sources.app_run_hash(image, 0)
            self.assertLess(tracemalloc.get_traced_memory()[1], 3 * sources.MAX_RUNTIME_BYTES)
        finally:
            tracemalloc.stop()

    def test_store_listing_is_bounded_before_parsing(self) -> None:
        image = self.assets / 'garmin-toolkit-x86_64.AppImage'
        with patch.object(sources, 'MAX_LISTING_BYTES', 64), self.assertRaisesRegex(ValueError, 'size limit'):
            sources.measured_store_paths(image, len(PREFIX))

    def test_inspection_failure_overflow_and_timeout_reap_the_child(self) -> None:
        children: list[subprocess.Popen[bytes]] = []
        popen = subprocess.Popen
        for code, exception, timeout in [
            ('import sys; sys.exit(7)', subprocess.CalledProcessError, 10),
            ('import sys; sys.stderr.write("x" * 262144)', ValueError, 10),
            ('import time; time.sleep(60)', subprocess.TimeoutExpired, 0.05),
        ]:
            with self.subTest(code=code):
                children.clear()

                def start(
                    _command: list[str], *, stdout: int, stderr: int, program: str = code
                ) -> subprocess.Popen[bytes]:
                    child = popen([sys.executable, '-c', program], stdout=stdout, stderr=stderr)
                    children.append(child)
                    return child

                with (
                    patch.object(sources.subprocess, 'Popen', side_effect=start),
                    patch.object(sources, 'INSPECTION_TIMEOUT', timeout),
                    self.assertRaises(exception),
                ):
                    sources.app_run_hash(self.root / 'unused', 0)
                self.assertEqual(len(children), 1)
                self.assertIsNotNone(children[0].poll())

    def setUp(self) -> None:
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        tree = self.root / 'tree'
        store = tree / STORE_PATH.removeprefix('/')
        store.mkdir(parents=True)
        (store / 'fixture').write_text('Synthetic bundled library\n')
        (tree / 'AppRun').write_bytes(APP_RUN)
        payload = self.root / 'payload.squashfs'
        subprocess.run(
            [
                'mksquashfs',
                str(tree),
                str(payload),
                '-noappend',
                '-processors',
                '1',
                '-all-root',
                '-no-progress',
                '-no-xattrs',
            ],
            check=True,
            capture_output=True,
        )
        self.assets = self.root / 'assets'
        self.assets.mkdir()
        for architecture in ('x86_64', 'aarch64'):
            for mode in ('garmin-toolkit', 'garmin-toolkit-demo'):
                (self.assets / f'{mode}-{architecture}.AppImage').write_bytes(PREFIX + payload.read_bytes())
        self.fixture_sources()

    def fixture_sources(self) -> None:
        source_assets(self.assets, TAG, COMMIT, offset=len(PREFIX), app_run_sha256=hashlib.sha256(APP_RUN).hexdigest())

    def test_measures_real_squashfs_and_checks_both_modes_without_executing_them(self) -> None:
        for architecture in ('x86_64', 'aarch64'):
            sources.verify(self.assets, architecture, TAG, COMMIT)

    def test_omitting_a_static_runtime_provider_is_rejected(self) -> None:
        report = self.assets / sources.names('aarch64')[1]
        document = object_value(read_json(report))
        document['runtime'] = [row for row in object_rows(document['runtime']) if row['path'] != STATIC_LIBRARY]
        report.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'complete AppImage runtime'):
            sources.verify(self.assets, 'aarch64', TAG, COMMIT)

    def test_static_library_inventory_cannot_be_empty(self) -> None:
        path = self.assets / sources.names('aarch64')[0]
        inventory = object_value(read_json(path))
        image = object_value(object_value(inventory['images'])['garmin-toolkit-aarch64.AppImage'])
        image['static_libraries'] = []
        path.write_bytes(json_bytes(inventory))
        with self.assertRaisesRegex(ValueError, 'static library inventory'):
            sources.verify(self.assets, 'aarch64', TAG, COMMIT)

    def test_wrong_app_run_or_runtime_prefix_is_rejected_even_with_updated_image_hash(self) -> None:
        for field in ('runtime_sha256', 'app_run_sha256'):
            with self.subTest(field=field):
                self.fixture_sources()
                inventory_path = self.assets / sources.names('x86_64')[0]
                inventory = object_value(read_json(inventory_path))
                image = object_value(object_value(inventory['images'])['garmin-toolkit-x86_64.AppImage'])
                image[field] = '0' * 64
                inventory_path.write_bytes(json_bytes(inventory))
                with self.assertRaisesRegex(ValueError, 'runtime or AppRun'):
                    sources.verify(self.assets, 'x86_64', TAG, COMMIT)

    def test_inventory_cannot_omit_store_paths_or_belong_to_another_commit(self) -> None:
        inventory_path = self.assets / sources.names('x86_64')[0]
        inventory = object_value(read_json(inventory_path))
        inventory['commit'] = '2' * 40
        inventory_path.write_bytes(json_bytes(inventory))
        with self.assertRaisesRegex(ValueError, 'pinned source'):
            sources.verify(self.assets, 'x86_64', TAG, COMMIT)
        inventory['commit'] = COMMIT
        image = object_value(object_value(inventory['images'])['garmin-toolkit-x86_64.AppImage'])
        image['store_paths'] = []
        inventory_path.write_bytes(json_bytes(inventory))
        with self.assertRaisesRegex(ValueError, 'measured inventory'):
            sources.verify(self.assets, 'x86_64', TAG, COMMIT)

    def test_unresolved_licenses_block_verification(self) -> None:
        report = self.assets / sources.names('x86_64')[1]
        document = object_value(read_json(report))
        runtime = object_rows(document['runtime'])
        runtime[0].update(licenses=[], resolved=False)
        document['unresolved'] = [runtime[0]['path']]
        report.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'unresolved'):
            sources.verify(self.assets, 'x86_64', TAG, COMMIT)

    def test_split_archives_are_verified_and_missing_or_tampered_parts_fail(self) -> None:
        with patch('hass_sources.MAX_BUNDLE_BYTES', 128):
            self.fixture_sources()
        sources.verify(self.assets, 'x86_64', TAG, COMMIT)
        self.assertGreater(len(sources.asset_names(self.assets, 'x86_64')), 3)
        archive = self.assets / (sources.names('x86_64')[2] + '.002')
        archive.write_bytes(b'corrupt')
        with self.assertRaises(ValueError):
            sources.verify(self.assets, 'x86_64', TAG, COMMIT)

    def test_source_archive_is_bound_to_the_runtime_inventory_bytes(self) -> None:
        report = self.assets / sources.names('aarch64')[1]
        document = object_value(read_json(report))
        self.assertEqual(document['image_sha256'], file_sha256(self.assets / sources.names('aarch64')[0]))
        document['image_sha256'] = '0' * 64
        report.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'differs from its image'):
            sources.verify(self.assets, 'aarch64', TAG, COMMIT)


if __name__ == '__main__':
    unittest.main()
