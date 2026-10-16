"""Regression coverage for rebuilding outputs originally copied from the Nix store."""

import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from prepare_cargo_target import prepare


class PrepareCargoTargetTests(unittest.TestCase):
    def test_just_prepares_outputs_before_starting_the_requested_command(self) -> None:
        just = shutil.which('just')
        assert just is not None
        source_root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / 'infra/just'
            scripts.mkdir(parents=True)
            shutil.copy2(source_root / 'infra/just/common.just', scripts / 'common.just')
            wrapper = scripts / 'memory-capped.sh'
            wrapper.write_text('exec "$@"\n')
            tools = root / 'infra/python'
            tools.mkdir()
            shutil.copy2(source_root / 'infra/python/prepare_cargo_target.py', tools)
            binaries = root / 'bin'
            binaries.mkdir()
            (binaries / 'python3').symlink_to(sys.executable)
            binding = root / '.tmp/cargo-target/debug/build/sqlite/out/bindgen.rs'
            binding.parent.mkdir(parents=True)
            binding.write_text('old')
            binding.chmod(0o444)

            result = subprocess.run(
                [
                    just,
                    '--tempdir',
                    directory,
                    '--justfile',
                    str(scripts / 'common.just'),
                    'in-shell',
                    'python3',
                    '-c',
                    'from pathlib import Path; import sys; Path(sys.argv[1]).write_text("rebuilt")',
                    str(binding),
                ],
                cwd=directory,
                env={
                    **{key: value for key, value in os.environ.items() if key != 'CARGO_TARGET_DIR'},
                    'PATH': f'{binaries}:{os.environ["PATH"]}',
                },
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(binding.read_text(), 'rebuilt')

    def test_readonly_generated_binding_can_be_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source.rs'
            source.write_text('old binding')
            source.chmod(0o444)
            for profile in ('debug', 'release', 'wasm32-unknown-unknown/debug'):
                output = root / '.tmp/cargo-target' / profile / 'build/sqlite-hash/out'
                output.mkdir(parents=True)
                binding = output / 'bindgen.rs'
                shutil.copy2(source, binding)
                if os.geteuid() != 0:
                    with self.assertRaises(PermissionError):
                        shutil.copyfile(source, binding)
                output.chmod(0o555)

            prepare(root, Path('.tmp/cargo-target'))

            for binding in (root / '.tmp/cargo-target').rglob('bindgen.rs'):
                self.assertEqual(stat.S_IMODE(binding.stat().st_mode), 0o644)
                self.assertEqual(stat.S_IMODE(binding.parent.stat().st_mode), 0o755)
                shutil.copyfile(source, binding)
                self.assertEqual(binding.read_text(), 'old binding')
            self.assertEqual(stat.S_IMODE(source.stat().st_mode), 0o444)

    def test_preserves_other_artifacts_and_custom_targets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for target in ('cargo-target', 'gallery-target', 'build-profile/nightly-target', 'custom'):
                cache = root / '.tmp' / target
                output = cache / 'debug/build/dep/out'
                output.mkdir(parents=True)
                generated = output / 'generated'
                generated.write_text('data')
                generated.chmod(0o555)
                artifact = cache / 'debug/library.rlib'
                artifact.write_text('keep')
                artifact.chmod(0o444)

                prepare(root, cache)

                expected = 0o555 if target == 'custom' else 0o755
                self.assertEqual(stat.S_IMODE(generated.stat().st_mode), expected)
                self.assertEqual(stat.S_IMODE(artifact.stat().st_mode), 0o444)

    def test_does_not_follow_output_or_ancestor_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / '.tmp/cargo-target'
            outside = root / 'outside'
            outside.mkdir()
            protected = outside / 'protected'
            protected.write_text('keep')
            protected.chmod(0o444)
            build = target / 'debug/build'
            build.mkdir(parents=True)
            (build / 'linked').symlink_to(outside, target_is_directory=True)
            output = build / 'normal/out'
            output.mkdir(parents=True)
            (output / 'file').symlink_to(protected)
            (output / 'directory').symlink_to(outside, target_is_directory=True)
            (target / 'linked-profile').symlink_to(outside, target_is_directory=True)

            prepare(root, target)

            self.assertEqual(stat.S_IMODE(protected.stat().st_mode), 0o444)
            shutil.rmtree(target)
            target.symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'symlinked Cargo target'):
                prepare(root, target)
            self.assertEqual(stat.S_IMODE(protected.stat().st_mode), 0o444)

    def test_rejects_readonly_hardlinks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            protected = root / 'source'
            protected.write_text('keep')
            protected.chmod(0o444)
            output = root / '.tmp/cargo-target/debug/build/dep/out'
            output.mkdir(parents=True)
            (output / 'binding').hardlink_to(protected)

            with self.assertRaisesRegex(ValueError, 'hard-linked Cargo output'):
                prepare(root, Path('.tmp/cargo-target'))

            self.assertEqual(stat.S_IMODE(protected.stat().st_mode), 0o444)


if __name__ == '__main__':
    unittest.main()
