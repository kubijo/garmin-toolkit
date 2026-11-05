"""Run inside the non-Nix Ubuntu desktop session, using its native D-Bus and graphics environment."""

import hashlib
import json
import platform
import subprocess
import time
from pathlib import Path

HOME = Path.home()


def run() -> None:
    deadline = time.monotonic() + 1800
    manifest = HOME / 'input.json'
    while not manifest.exists():
        if time.monotonic() >= deadline:
            raise TimeoutError('AppImage was not delivered')
        time.sleep(0.5)
    source: dict[str, str] = json.loads(manifest.read_text())
    app = HOME / 'garmin-toolkit.AppImage'
    if (
        platform.machine() != 'aarch64'
        or Path('/nix').exists()
        or platform.freedesktop_os_release().get('VERSION_ID') != '26.04'
    ):
        raise ValueError('Acceptance requires Ubuntu 26.04 aarch64 without a Nix store')
    with app.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        stream.seek(0)
        header = stream.read(20)
    if digest != source['sha256']:
        raise ValueError('Delivered AppImage hash differs from the build input')
    if (
        len(header) != 20
        or header[:6] != b'\x7fELF\x02\x01'
        or header[8:11] != b'AI\x02'
        or int.from_bytes(header[18:20], 'little') != 183
    ):
        raise ValueError('Artifact must be an ARM64 little-endian type-2 AppImage')
    app.chmod(0o755)
    with (HOME / 'app.log').open('w') as log:
        process = subprocess.Popen([str(app)], stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 120
            while True:
                if process.poll() is not None:
                    raise ValueError(f'AppImage exited during startup: {process.returncode}')
                windows = subprocess.run(['wmctrl', '-l'], capture_output=True, text=True, check=True).stdout
                if any(line.split(None, 3)[-1] == 'Garmin Toolkit' for line in windows.splitlines()):
                    break
                if time.monotonic() >= deadline:
                    raise TimeoutError('No Garmin Toolkit window appeared')
                time.sleep(0.5)
            packages = subprocess.run(
                ['dpkg-query', '-W', '-f', '${Package}\t${Version}\n'], capture_output=True, text=True, check=True
            ).stdout
            (HOME / 'packages.txt').write_text(packages)
            result = {'architecture': platform.machine(), 'sha256': digest, 'pid': process.pid, 'window': windows}
            (HOME / 'ready.pending').write_text(json.dumps(result, indent=2) + '\n')
            (HOME / 'ready.pending').replace(HOME / 'ready.json')
            status = process.wait()
            raise ValueError(f'AppImage exited before session cleanup: {status}')
        finally:
            (HOME / 'ready.json').unlink(missing_ok=True)
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    try:
        run()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        (HOME / 'error.txt').write_text(str(error) + '\n')
        raise
