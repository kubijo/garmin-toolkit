"""Keep generated build outputs writable when sharing Cargo artifacts with Nix."""

import os
import stat
import sys
from pathlib import Path


def prepare(root: Path, target: Path) -> None:
    """Repair only known repository targets, without following links outside them."""
    if not target.is_absolute():
        target = root / target
    allowed = (
        root / '.tmp/cargo-target',
        root / '.tmp/gallery-target',
        root / '.tmp/build-profile/nightly-target',
    )
    if target not in allowed:
        return
    for path in (target, *target.parents):
        if path == root:
            break
        if path.is_symlink():
            raise ValueError(f'refusing symlinked Cargo target parent: {path}')
    if not target.exists():
        return

    for build in (*target.glob('*/build'), *target.glob('*/*/build')):
        if any(path.is_symlink() for path in (build, *build.parents) if path.is_relative_to(target)):
            continue
        for output in build.glob('*/out'):
            if output.parent.is_symlink() or output.is_symlink():
                continue
            for directory, children, files in os.walk(output, followlinks=False):
                parent = Path(directory)
                children[:] = [name for name in children if not (parent / name).is_symlink()]
                make_writable(parent)
                for name in files:
                    path = parent / name
                    if not path.is_symlink():
                        make_writable(path)


def make_writable(path: Path) -> None:
    """Preserve existing mode bits and avoid changing hard-linked source files."""
    metadata = path.stat()
    if metadata.st_mode & stat.S_IWUSR:
        return
    if stat.S_ISREG(metadata.st_mode) and metadata.st_nlink != 1:
        raise ValueError(f'refusing hard-linked Cargo output: {path}')
    if stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode):
        path.chmod(metadata.st_mode | stat.S_IWUSR)


def main() -> int:
    root = Path.cwd()
    target = Path(os.environ.get('CARGO_TARGET_DIR', '.tmp/cargo-target'))
    try:
        prepare(root, target)
    except (OSError, ValueError) as error:
        print(f'Cargo target preparation failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
