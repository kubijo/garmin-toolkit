"""Shared pinned images, resource limits, and process ownership for HAOS runners."""

import contextlib
import hashlib
import lzma
import os
import shutil
import subprocess
import time
from pathlib import Path

from haos_platform import Platform, machine_arguments
from processes import ACCEPTANCE_SCOPE, stop_group, vm_scope

MAX_COMPRESSED_BYTES = 600_000_000
MAX_MEMORY_BYTES = 6 * 1024**3
MAX_CPUS = 4
SCRATCH_ROOT = Path(__file__).resolve().parents[2] / '.tmp'


def prepare_image(compressed: Path, destination: Path, guest: Platform) -> None:
    if compressed.stat().st_size > MAX_COMPRESSED_BYTES:
        raise ValueError('HAOS image exceeds the download size limit')
    with compressed.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    if digest != guest.image_sha256:
        raise ValueError(f'HAOS image SHA-256 differs from the pinned release: {digest}')
    with lzma.open(compressed, 'rb') as source, destination.open('xb') as output:
        shutil.copyfileobj(source, output, length=1024 * 1024)


def download_image(destination: Path, guest: Platform) -> None:
    from urllib.request import Request, urlopen

    request = Request(guest.image_url, headers={'User-Agent': 'garmin-toolkit-haos-app'})
    with urlopen(request, timeout=30) as source, destination.open('xb') as output:
        total = 0
        while chunk := source.read(1024 * 1024):
            total += len(chunk)
            if total > MAX_COMPRESSED_BYTES:
                raise ValueError('HAOS image exceeds the download size limit')
            output.write(chunk)


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
    try:
        with contextlib.suppress(OSError, subprocess.TimeoutExpired):
            subprocess.run(
                ['systemctl', '--user', 'stop', f'{unit}.scope'], check=False, timeout=15, capture_output=True
            )
    finally:
        stop_group(process)
        if process.stdout is not None:
            process.stdout.close()


def qemu_command(
    image: Path,
    config: Path | None,
    runtime: Path,
    unit: str,
    port: int,
    timeout: int,
    *,
    guest: Platform,
    overlay_limit: int,
) -> list[str]:
    command = ['systemd-run', '--user', '--scope', '--expand-environment=no', f'--unit={unit}']
    owner = os.environ.get(ACCEPTANCE_SCOPE)
    if owner is not None:
        if unit + '.scope' != vm_scope(owner):
            raise ValueError('Acceptance VM must use its assigned owned scope')
        command += ['-p', 'BindsTo=' + owner, '-p', 'After=' + owner, '-p', 'TimeoutStopSec=5s']
    command += ['-p', 'MemoryMax=6G', '-p', f'CPUQuota={MAX_CPUS * 100}%', '-p', f'RuntimeMaxSec={timeout + 60}']
    command += [
        'prlimit',
        f'--fsize={overlay_limit}',
        '--',
        'timeout',
        '--signal=TERM',
        '--kill-after=10s',
        f'{timeout}s',
    ]
    command += machine_arguments(guest, runtime, force_tcg=config is None)
    command += ['-smp', str(MAX_CPUS), '-m', '3072' if config is None else '4096']
    command += ['-drive', f'if=none,id=haos,file={image},format=qcow2,cache=none']
    command += ['-device', 'virtio-blk-pci,drive=haos,bootindex=0', '-device', 'qemu-xhci', '-monitor', 'none']
    command += ['-serial', f'file:{runtime / "serial.log"}']
    if config is None:
        command += ['-nic', 'none', '-vnc', f'unix:{runtime / "vnc.sock"}']
        command += ['-qmp', f'unix:{runtime / "qmp.sock"},server=on,wait=off', '-no-reboot']
    else:
        command += ['-drive', f'if=none,id=config,file={config},format=raw,readonly=on']
        command += ['-device', 'usb-storage,drive=config']
        command += ['-netdev', f'user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22222']
        command += ['-device', 'virtio-net-pci,netdev=net0', '-display', 'none']
    return command
