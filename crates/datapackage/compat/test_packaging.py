"""Prove Cargo's distribution archive builds without this repository or its paths."""

import os
import subprocess
import tarfile
import tempfile
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PackagingTests(unittest.TestCase):
    def test_distribution_archive_is_independent(self) -> None:
        target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')).resolve()
        subprocess.run(
            [
                'cargo',
                'package',
                '--manifest-path',
                str(ROOT / 'Cargo.toml'),
                '--allow-dirty',
                '--no-verify',
                '--offline',
                '--target-dir',
                str(target),
            ],
            check=True,
        )
        version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
        archive = target / 'package' / f'datapackage-{version}.crate'
        # QA's TMPDIR is inside the application checkout; this copy must be outside it.
        with tempfile.TemporaryDirectory(
            prefix='datapackage-standalone-', dir=os.environ.get('TEMP') or '/tmp'
        ) as directory:
            with tarfile.open(archive) as source:
                source.extractall(directory, filter='data')
            package = Path(directory) / f'datapackage-{version}'
            metadata = tomllib.loads((package / 'Cargo.toml').read_text())
            for attributes in metadata['dependencies'].values():
                self.assertNotIn('path', attributes)
                self.assertNotIn('workspace', attributes)
            self.assertTrue((package / 'LICENSE').is_file())
            self.assertTrue((package / 'tests/upstream/manifest.json').is_file())
            self.assertTrue((package / 'compat/pyproject.toml').is_file())
            self.assertTrue((package / 'compat/uv.lock').is_file())
            subprocess.run(
                [
                    'cargo',
                    'test',
                    '--manifest-path',
                    str(package / 'Cargo.toml'),
                    '--offline',
                    '--target-dir',
                    str(Path(directory) / 'target'),
                ],
                cwd=package,
                check=True,
            )


if __name__ == '__main__':
    unittest.main()
