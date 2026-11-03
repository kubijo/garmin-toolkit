"""Exercise dependency upgrades without inventing or silently changing upstream notices."""

import hashlib
import json
import tempfile
import tomllib
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import patch

from sync import source_for, synchronize


class SynchronizationTests(unittest.TestCase):
    def test_gitlab_source_keeps_repository_and_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, packages = self.fixture(root)
            package = packages[('dependency', '2')]
            package['repository'] = 'https://gitlab.com/example/project.git'
            old = 'a' * 40
            new = 'b' * 40
            self.assertEqual(
                source_for(package, f'https://gitlab.com/example/project/-/raw/{old}/Cargo.toml'),
                f'https://gitlab.com/example/project/-/raw/{new}/Cargo.toml',
            )
            self.assertIsNone(source_for(package, f'https://gitlab.com/unrelated/project/-/raw/{old}/Cargo.toml'))

    def test_unchanged_declaration_uses_existing_origin_verifier(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, packages = self.fixture(root)
            with manifest.open('a') as stream:
                stream.write('\n[[declarations]]\npackage = "dependency 2"\n')
            with (
                patch('sync.fetch_notice', return_value=(root / 'LICENSE.txt').read_bytes()),
                patch('sync.verified_declaration') as verify,
            ):
                synchronize(packages, manifest)
            verify.assert_called_once_with(packages[('dependency', '2')], {'package': 'dependency 2'})

    def fixture(self, root: Path) -> tuple[Path, dict[tuple[str, str], dict[str, Any]]]:
        notice = b'Copyright upstream author\nActual license terms\n'
        (root / 'LICENSE.txt').write_bytes(notice)
        old = 'a' * 40
        source = f'https://raw.githubusercontent.com/example/project/{old}/LICENSE'
        manifest = root / 'notices.toml'
        manifest.write_text(
            '[[notices]]\npackages = ["dependency 1"]\nlicense = "MIT"\n'
            f'file = "LICENSE.txt"\nsource = "{source}"\n'
            f'sha256 = "{hashlib.sha256(notice).hexdigest()}"\n'
        )
        packages: dict[tuple[str, str], dict[str, Any]] = {}
        for name, version in [('dependency', '2'), ('new-sibling', '1')]:
            directory = root / name
            directory.mkdir()
            (directory / '.cargo_vcs_info.json').write_text(json.dumps({'git': {'sha1': 'b' * 40}}))
            packages[(name, version)] = {
                'name': name,
                'version': version,
                'license': 'MIT',
                'repository': 'https://github.com/example/project',
                'manifest_path': str(directory / 'Cargo.toml'),
            }
        return manifest, packages

    def test_upgrade_and_new_sibling_preserve_verified_notice(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, packages = self.fixture(root)
            with patch('sync.fetch_notice', return_value=(root / 'LICENSE.txt').read_bytes()) as fetch:
                synchronize(packages, manifest)
            configured = tomllib.loads(manifest.read_text())
            self.assertEqual(
                {identifier for record in configured['notices'] for identifier in record['packages']},
                {'dependency 2', 'new-sibling 1'},
            )
            self.assertTrue(all('b' * 40 in record['source'] for record in configured['notices']))
            self.assertEqual(fetch.call_count, 1)
            before = manifest.read_bytes()
            with patch('sync.fetch_notice') as fetch:
                synchronize(packages, manifest)
            fetch.assert_not_called()
            self.assertEqual(manifest.read_bytes(), before)

    def test_changed_upstream_text_is_extracted_without_overwriting_shared_notice(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            manifest, packages = self.fixture(Path(directory))
            original = (Path(directory) / 'LICENSE.txt').read_bytes()
            upstream = b'Different copyright and terms'
            with patch('sync.fetch_notice', return_value=upstream):
                synchronize(packages, manifest)
            records = tomllib.loads(manifest.read_text())['notices']
            for record in records:
                self.assertEqual(record['sha256'], hashlib.sha256(upstream).hexdigest())
                self.assertEqual((Path(directory) / record['file']).read_bytes(), upstream + b'\n')
                self.assertTrue(record['no_final_newline'])
            self.assertEqual((Path(directory) / 'LICENSE.txt').read_bytes(), original)

    def test_unrelated_repository_cannot_inherit_notice(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            manifest, packages = self.fixture(Path(directory))
            package = packages[('new-sibling', '1')]
            package['repository'] = 'https://github.com/unrelated/project'
            with patch('sync.fetch_notice', return_value=(Path(directory) / 'LICENSE.txt').read_bytes()):
                synchronize(packages, manifest)
            identifiers = {
                identifier
                for record in tomllib.loads(manifest.read_text())['notices']
                for identifier in record['packages']
            }
            self.assertNotIn('new-sibling 1', identifiers)


if __name__ == '__main__':
    unittest.main()
