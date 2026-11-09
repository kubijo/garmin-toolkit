"""Repository-owned SDK walkthrough; Garmin agreement and login stay in Garmin's UI."""

import platform
import subprocess
import sys
import uuid
from pathlib import Path

import connect_iq as local
from connect_iq_macos import install_manager
from connect_iq_manager import Runtime, extract_manager
from json_data import json_bytes, object_value, read_json, text


def selected_sdk(home: Path, system: str) -> Path | None:
    if system == 'Linux':
        base = home / '.Garmin/ConnectIQ'
    elif system == 'Darwin':
        base = home / 'Library/Application Support/Garmin/ConnectIQ'
    else:
        raise ValueError('Connect IQ setup supports Linux and macOS')
    config = base / 'current-sdk.cfg'
    if not config.is_file():
        return None
    value = config.read_text().strip()
    path = Path(value)
    if not value or not path.is_absolute() or '\n' in value:
        raise ValueError('SDK Manager current-sdk.cfg must contain one absolute SDK path')
    return local.local_material(path)


def prepare(*, archive: Path) -> Path:
    """Install an owner-downloaded Manager ZIP/DMG locally without opening its GUI."""
    state = local.ROOT / '.tmp/connect-iq'
    if platform.system() == 'Darwin':
        manager = install_manager(archive, state)
    else:
        manager = extract_manager(archive, state)
        runtime = Runtime(local.ROOT, state)
        runtime.ensure_image()
        runtime.verify(manager)
    local.OUTPUT.panel(str(manager), title='SDK Manager prepared')
    return manager


def setup(
    *, archive: Path | None = None, manager: Path | None = None, sdk: Path | None = None, key: Path | None = None
) -> None:
    """Own the local walkthrough through Manager, signing key, validation and the first build."""
    if archive is not None and manager is not None:
        raise ValueError('Select either a Manager archive or an installed Manager')
    system = platform.system()
    state = local.ROOT / '.tmp/connect-iq'
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    selected = sdk or (None if archive is not None or manager is not None else selected_sdk(Path.home(), system))
    if selected is None:
        filename = 'connectiq-sdk-manager.dmg' if system == 'Darwin' else 'connectiq-sdk-manager-linux.zip'
        default_archive = local.ROOT / '.tmp' / filename
        if archive is None and manager is None and default_archive.is_file():
            archive = default_archive
        if archive is None and manager is None:
            local.bootstrap()
            if not sys.stdin.isatty():
                raise ValueError('Supply --archive /path/to/Manager.zip-or-dmg or --manager /path/to/SDKManager.app')
            path = Path(
                input('Path to your downloaded Linux ZIP or installed macOS SDKManager.app: ').strip()
            ).expanduser()
            archive, manager = (path, None) if path.suffix in ('.zip', '.dmg') else (None, path)
        local.OUTPUT.panel(
            'In SDK Manager: sign in yourself, install an SDK, make it active, and install the\n'
            'fenix8solar47mm device definition. Then close SDK Manager to continue the build.',
            title='Owner-operated Garmin setup',
        )
        if system == 'Linux':
            if archive is not None:
                manager = prepare(archive=archive)
            elif manager is not None:
                runtime = Runtime(local.ROOT, state)
                runtime.ensure_image()
                runtime.verify(manager.resolve())
            if manager is None:
                raise ValueError('A Linux SDK Manager is required')
            Runtime(local.ROOT, state).launch(manager.resolve())
        elif system == 'Darwin':
            if archive is not None:
                manager = prepare(archive=archive)
            if manager is None:
                raise ValueError('A macOS SDK Manager is required')
            subprocess.run(['/usr/bin/open', '-W', '-a', str(manager.resolve())], check=True, timeout=3600)
        else:
            raise ValueError('Supply an installed SDKManager.app on macOS or --sdk for an existing SDK')
        selected = selected_sdk(Path.home(), system)
    if selected is None:
        raise ValueError('No active SDK selected; finish SDK Manager setup and retry')
    selected_key = key or state / 'developer.der'
    if key is None and (state / 'local.json').is_file():
        settings = object_value(read_json(state / 'local.json', max_bytes=4096))
        selected_key = Path(text(settings.get('key')))
    local.keygen(key=selected_key)
    (state / 'local.json').write_bytes(json_bytes({'sdk': str(selected.resolve()), 'key': str(selected_key.resolve())}))
    rebuild()


def rebuild(*, output: Path | None = None, device: local.Device = 'fenix8solar47mm') -> None:
    """Build with the SDK and key retained by setup; outputs never replace a previous build."""
    state = local.ROOT / '.tmp/connect-iq'
    settings = object_value(read_json(state / 'local.json', max_bytes=4096))
    sdk, key = Path(text(settings.get('sdk'))), Path(text(settings.get('key')))
    local.build(sdk=sdk, key=key, output=output or state / ('build-' + uuid.uuid4().hex), device=device)
