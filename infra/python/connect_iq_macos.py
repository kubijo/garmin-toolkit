"""Install an owner-downloaded Manager DMG locally using native macOS tools."""

import hashlib
import plistlib
import subprocess
import tempfile
import uuid
from pathlib import Path

from json_data import object_rows, object_value, text
from processes import cleanup_signals


def install_manager(image: Path, state: Path) -> Path:
    with image.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    mount = state / ('dmg-' + uuid.uuid4().hex)
    mount.mkdir(parents=True, mode=0o700)
    attached = False
    try:
        result = subprocess.run(
            [
                '/usr/bin/hdiutil',
                'attach',
                '-readonly',
                '-nobrowse',
                '-plist',
                '-mountpoint',
                str(mount),
                str(image.resolve()),
            ],
            check=True,
            capture_output=True,
            timeout=120,
        )
        attached = True
        entities = object_rows(object_value(plistlib.loads(result.stdout)).get('system-entities'))
        points = [Path(text(entity['mount-point'])) for entity in entities if 'mount-point' in entity]
        if points != [mount]:
            raise ValueError('DMG did not mount at the owned setup mountpoint')
        apps = list(mount.glob('*.app'))
        if len(apps) != 1 or not apps[0].is_dir() or apps[0].is_symlink():
            raise ValueError('Expected exactly one SDK Manager app at the DMG root')
        destination = state / 'manager' / digest / apps[0].name
        if destination.is_symlink():
            raise ValueError('Manager destination must not be a symbolic link')
        if destination.exists() and not destination.is_dir():
            raise ValueError('Manager destination must be an application directory')
        if not destination.exists():
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            with tempfile.TemporaryDirectory(dir=destination.parent, prefix='install-') as temporary:
                staged = Path(temporary) / apps[0].name
                subprocess.run(['/usr/bin/ditto', str(apps[0]), str(staged)], check=True, timeout=120)
                staged.rename(destination)
        return destination
    finally:
        with cleanup_signals():
            if attached:
                subprocess.run(['/usr/bin/hdiutil', 'detach', str(mount)], check=True, timeout=60)
            mount.rmdir()
