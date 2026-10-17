"""The manual cache cleaner may only remove named target directories."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SOURCE = Path(__file__).resolve().parents[1] / 'just' / 'build-cache.sh'


class BuildCacheTests(unittest.TestCase):
    def test_report_and_dry_run_preserve_caches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root, tool = self.fixture(Path(directory))
            shared = root / '.tmp/cargo-target'
            shared.mkdir(parents=True)
            (shared / 'artifact').write_text('keep')

            report = self.run_tool(tool, 'report', home=root)
            self.assertIn('shared', report.stdout)
            self.assertIn('all .tmp', report.stdout)
            preview = self.run_tool(tool, 'clean', 'shared', 'dry-run', home=root)
            self.assertIn(str(shared), preview.stdout)
            self.assertTrue((shared / 'artifact').exists())

    def test_apply_removes_only_selected_cache(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root, tool = self.fixture(Path(directory))
            shared = root / '.tmp/cargo-target'
            gallery = root / '.tmp/gallery-target'
            for path in (shared, gallery):
                path.mkdir(parents=True)
                (path / 'artifact').write_text('keep')

            self.run_tool(tool, 'clean', 'shared', 'apply', home=root)
            self.assertFalse(shared.exists())
            self.assertTrue((gallery / 'artifact').exists())

    def test_rejects_unknown_name_and_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root, tool = self.fixture(Path(directory))
            outside = root / 'outside'
            outside.mkdir()
            (outside / 'artifact').write_text('keep')
            temporary = root / '.tmp'
            temporary.mkdir()
            (temporary / 'cargo-target').symlink_to(outside, target_is_directory=True)

            unknown = self.run_tool(tool, 'clean', 'outside', 'apply', home=root, check=False)
            self.assertEqual(unknown.returncode, 64)
            linked = self.run_tool(tool, 'clean', 'shared', 'apply', home=root, check=False)
            self.assertNotEqual(linked.returncode, 0)
            self.assertTrue((outside / 'artifact').exists())

    def test_rejects_symlinked_parent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root, tool = self.fixture(Path(directory))
            outside = root / 'outside'
            (outside / 'cargo-target').mkdir(parents=True)
            (outside / 'cargo-target/artifact').write_text('keep')
            (root / '.tmp').symlink_to(outside, target_is_directory=True)

            result = self.run_tool(tool, 'clean', 'shared', 'apply', home=root, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue((outside / 'cargo-target/artifact').exists())

    @staticmethod
    def fixture(root: Path) -> tuple[Path, Path]:
        tool = root / 'infra/just/build-cache.sh'
        tool.parent.mkdir(parents=True)
        shutil.copy2(SOURCE, tool)
        return root, tool

    @staticmethod
    def run_tool(tool: Path, *arguments: str, home: Path, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ['bash', str(tool), *arguments],
            env={**os.environ, 'HOME': str(home)},
            capture_output=True,
            text=True,
            check=check,
        )


if __name__ == '__main__':
    unittest.main()
