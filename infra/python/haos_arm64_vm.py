"""Boot an isolated Home Assistant OS ARM64 image through QEMU TCG."""

import contextlib
import hashlib
import lzma
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path

import tyro
from rich.text import Text

from cli_output import TerminalOutput

OUTPUT = TerminalOutput()
IMAGE_URL = (
    'https://github.com/home-assistant/operating-system/releases/download/18.3/haos_generic-aarch64-18.3.qcow2.xz'
)
IMAGE_SHA256 = 'f54b659f12c665038134bdf9d4235903048056257368f82369dc230131f5a675'
MAX_COMPRESSED_BYTES = 400_000_000
MAX_OVERLAY_BYTES = 2 * 1024**3
MAX_MEMORY_BYTES = 6 * 1024**3
MAX_CPUS = 4
FIRMWARE = Path('/usr/share/qemu-efi-aarch64/QEMU_EFI.fd')
SCRATCH_ROOT = Path(__file__).resolve().parents[2] / '.tmp'


@dataclass(frozen=True)
class Options:
    """Smoke-test ARM64 HAOS boot without network, USB, or persistent state."""

    image: Path | None = None
    """Official compressed QCOW2 image; download it into temporary storage when omitted."""

    timeout: int = 360
    """Maximum boot time in seconds."""


def download_image(destination: Path) -> None:
    request = urllib.request.Request(IMAGE_URL, headers={'User-Agent': 'garmin-toolkit-haos-smoke'})
    with urllib.request.urlopen(request, timeout=30) as source, destination.open('xb') as output:
        total = 0
        while chunk := source.read(1024 * 1024):
            total += len(chunk)
            if total > MAX_COMPRESSED_BYTES:
                raise ValueError('HAOS image exceeds the download size limit')
            output.write(chunk)


def unpack_image(compressed: Path, destination: Path) -> None:
    if compressed.stat().st_size > MAX_COMPRESSED_BYTES:
        raise ValueError('HAOS image exceeds the download size limit')
    with compressed.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    if digest != IMAGE_SHA256:
        raise ValueError(f'HAOS image SHA-256 differs from the pinned release: {digest}')
    with lzma.open(compressed, 'rb') as source, destination.open('xb') as output:
        shutil.copyfileobj(source, output, length=1024 * 1024)


def qemu_command(image: Path, runtime: Path, unit: str, timeout: int) -> list[str]:
    return [
        'systemd-run',
        '--user',
        '--scope',
        f'--unit={unit}',
        '-p',
        'MemoryMax=6G',
        '-p',
        f'CPUQuota={MAX_CPUS * 100}%',
        '-p',
        f'RuntimeMaxSec={timeout + 30}',
        'prlimit',
        f'--fsize={MAX_OVERLAY_BYTES}',
        '--',
        'timeout',
        '--signal=TERM',
        '--kill-after=10s',
        f'{timeout}s',
        'qemu-system-aarch64',
        '-machine',
        'virt',
        '-accel',
        'tcg,thread=multi',
        '-cpu',
        'cortex-a57',
        '-smp',
        str(MAX_CPUS),
        '-m',
        '3072',
        '-bios',
        str(FIRMWARE),
        '-drive',
        f'if=none,id=haos,file={image},format=qcow2',
        '-device',
        'virtio-blk-pci,drive=haos,bootindex=0',
        '-nic',
        'none',
        '-device',
        'qemu-xhci',
        '-device',
        'ramfb',
        '-vnc',
        f'unix:{runtime / "vnc.sock"}',
        '-qmp',
        f'unix:{runtime / "qmp.sock"},server=on,wait=off',
        '-monitor',
        'none',
        '-serial',
        f'file:{runtime / "serial.log"}',
        '-no-reboot',
    ]


def verify_scope(unit: str, deadline: float) -> None:
    while time.monotonic() < deadline:
        result = subprocess.run(
            ['systemctl', '--user', 'show', f'{unit}.scope', '--property=ControlGroup', '--value'],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        )
        group = result.stdout.strip()
        if result.returncode == 0 and group.startswith('/user.slice/'):
            path = Path('/sys/fs/cgroup') / group.lstrip('/')
            memory = int((path / 'memory.max').read_text().strip())
            quota, period = (path / 'cpu.max').read_text().split()
            if memory > MAX_MEMORY_BYTES or quota == 'max' or int(quota) > MAX_CPUS * int(period):
                raise RuntimeError('HAOS VM cgroup exceeds the four-CPU / six-GiB limit')
            return
        time.sleep(0.1)
    raise RuntimeError('Could not verify the HAOS VM cgroup limits')


def stop(process: subprocess.Popen[str], unit: str) -> None:
    with contextlib.suppress(OSError, subprocess.TimeoutExpired):
        subprocess.run(['systemctl', '--user', 'stop', f'{unit}.scope'], check=False, timeout=15, capture_output=True)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
    if process.stdout is not None:
        process.stdout.close()


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
        qemu_command(overlay, runtime, unit, timeout),
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
    if sys.platform != 'linux' or not FIRMWARE.is_file():
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
                download_image(compressed)
            image = runtime / 'haos.qcow2'
            unpack_image(compressed, image)
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
