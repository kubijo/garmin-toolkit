"""Boot an isolated Home Assistant OS ARM64 image through QEMU TCG."""

import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass
from pathlib import Path

import tyro
from rich.text import Text

from cli_output import TerminalOutput
from haos_platform import PLATFORMS
from haos_runtime import SCRATCH_ROOT, download_image, prepare_image, qemu_command, stop, verify_scope

OUTPUT = TerminalOutput()
GUEST = PLATFORMS['aarch64']
MAX_OVERLAY_BYTES = 2 * 1024**3


@dataclass(frozen=True)
class Options:
    """Smoke-test ARM64 HAOS boot without network, USB, or persistent state."""

    image: Path | None = None
    """Official compressed QCOW2 image; download it into temporary storage when omitted."""

    timeout: int = 360
    """Maximum boot time in seconds."""


def boot(image: Path, runtime: Path, timeout: int) -> None:
    overlay = runtime / 'overlay.qcow2'
    subprocess.run(
        ['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image), str(overlay)],
        check=True,
        capture_output=True,
        text=True,
        timeout=15,
    )
    unit = f'garmin-haos-arm64-{uuid.uuid4().hex[:12]}'
    process = subprocess.Popen(
        qemu_command(overlay, None, runtime, unit, 0, timeout, guest=GUEST, overlay_limit=MAX_OVERLAY_BYTES),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    try:
        deadline = time.monotonic() + timeout
        verify_scope(unit, min(deadline, time.monotonic() + 15))
        while time.monotonic() < deadline:
            if overlay.stat().st_size > MAX_OVERLAY_BYTES:
                raise RuntimeError('HAOS VM overlay exceeded the two-GiB smoke-test limit')
            if process.poll() is not None:
                output = process.stdout.read() if process.stdout else ''
                raise RuntimeError(f'HAOS VM exited before boot confirmation: {output.strip()}')
            serial = runtime / 'serial.log'
            if serial.exists():
                log = serial.read_text(errors='replace')
                if 'Booting `Slot A' in log and 'unit=haos-overlay' in log:
                    return
            time.sleep(0.5)
        serial = runtime / 'serial.log'
        tail = serial.read_text(errors='replace')[-2_000:] if serial.exists() else 'No serial output'
        raise TimeoutError(
            f'HAOS did not reach its data mount in {timeout}s. '
            f'Overlay size: {overlay.stat().st_size} bytes. Serial tail:\n{tail}'
        )
    finally:
        stop(process, unit)


def main(options: Options) -> int:
    if sys.platform != 'linux' or not GUEST.firmware.is_file():
        OUTPUT.panel(Text('Linux and QEMU ARM64 EFI firmware are required.'), title='VM unavailable', error=True)
        return 1
    if options.timeout < 10 or options.timeout > 600:
        OUTPUT.panel(Text('Timeout must be between 10 and 600 seconds.'), title='Invalid timeout', error=True)
        return 1
    try:
        SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='haos-arm64.', dir=SCRATCH_ROOT) as directory:
            runtime = Path(directory)
            compressed = options.image
            if compressed is None:
                compressed = runtime / 'haos.qcow2.xz'
                download_image(compressed, GUEST)
            image = runtime / 'haos.qcow2'
            prepare_image(compressed, image, GUEST)
            boot(image, runtime, options.timeout)
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError) as error:
        OUTPUT.panel(Text(str(error)), title='HAOS ARM64 boot failed', style='red', error=True)
        return 1
    OUTPUT.panel(
        Text('ARM64 HAOS reached its data mount. VM stopped and temporary files removed.'), title='Boot passed'
    )
    return 0


if __name__ == '__main__':
    OUTPUT.install_tracebacks()
    sys.exit(main(tyro.cli(Options)))
