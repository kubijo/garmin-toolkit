"""Build and qualify the ARM64 production AppImage with one selected USB device in disposable Ubuntu."""

import hashlib
import json
import platform
import re
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from collections.abc import Generator
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

import tyro
import yaml

from cli_output import TerminalOutput
from desktop_vm_qmp import Qmp
from json_data import Object, object_rows, object_value
from processes import stop_group, terminated

ROOT = Path.cwd()
SPEC = Path(__file__).resolve().parents[2] / 'infra/integration/desktop'
CASE = (
    (
        'discovery',
        'Create a test profile if needed. The selected Garmin appears under attached devices and inspection succeeds.',
    ),
    ('activities', 'Browse files → Activities lists the device FIT files and their sizes.'),
    ('preview', 'Double-click a FIT: its preview appears inside the file window with an explicit Import action.'),
    ('dismissal', 'Close the preview: the folder and selection are preserved, and no activity was imported.'),
)


@dataclass(frozen=True)
class Options:
    usb: str
    """Exactly one connected device, selected as hexadecimal vendor:product."""
    image: Path
    """Pinned Ubuntu cloud image, supplied by the Nix entrypoint."""
    firmware: Path
    machine: str
    appimage: Path | None = None
    """Use an existing production ARM64 AppImage instead of building the current checkout."""
    timeout: int = 3600
    """Maximum total guest preparation and acceptance time, in seconds."""
    output: Path = ROOT / '.tmp/desktop-vm'


def usb_identity(value: str) -> tuple[int, int]:
    if re.fullmatch(r'[0-9a-fA-F]{4}:[0-9a-fA-F]{4}', value) is None:
        raise ValueError('Select one USB device as vendor:product, for example 091e:0001')
    vendor, product = value.split(':')
    return int(vendor, 16), int(product, 16)


def port() -> int:
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def machine_arguments(system: str, host_arch: str, machine: str, firmware: Path) -> list[str]:
    if system not in {'Darwin', 'Linux'}:
        raise ValueError('Desktop VM acceptance supports macOS and Linux hosts')
    native = host_arch in {'arm64', 'aarch64'}
    accelerator = 'hvf' if system == 'Darwin' and native else 'kvm' if native else 'tcg,thread=multi'
    return [
        'qemu-system-aarch64',
        '-machine',
        machine,
        '-accel',
        accelerator,
        '-cpu',
        'host' if native else 'cortex-a72',
        '-bios',
        str(firmware),
        '-smp',
        '2',
        '-m',
        '4096',
    ]


def seed(public_key: str) -> Object:
    config = object_value(yaml.safe_load((SPEC / 'cloud-init.yaml').read_text()))
    user = object_rows(config['users'])[0]
    user['ssh_authorized_keys'] = [public_key.strip()]
    files = object_rows(config['write_files'])
    files.append({'path': '/opt/garmin-test/guest.py', 'content': (SPEC / 'guest.py').read_text()})
    config['users'] = [user]
    config['write_files'] = files
    return config


def build(appimage: Path | None, *, out_link: Path | None = None) -> Path:
    if appimage is not None:
        result = appimage.resolve()
    else:
        if out_link is None:
            raise ValueError('Building an AppImage requires an output link')
        subprocess.run(
            [
                'nix',
                'build',
                f'git+file://{ROOT}#packages.aarch64-linux.desktop-appimage',
                '--out-link',
                str(out_link),
                '--print-build-logs',
                '--no-update-lock-file',
            ],
            cwd=ROOT,
            check=True,
        )
        result = out_link.resolve(strict=True)
    if not result.is_file():
        raise ValueError('Expected exactly one AppImage file')
    with result.open('rb') as stream:
        header = stream.read(20)
    if (
        len(header) != 20
        or header[:6] != b'\x7fELF\x02\x01'
        or header[8:11] != b'AI\x02'
        or int.from_bytes(header[18:20], 'little') != 183
    ):
        raise ValueError('The acceptance artifact must be an ARM64 little-endian type-2 AppImage')
    return result


class Guest:
    def __init__(self, ssh_port: int, scratch: Path) -> None:
        self.options = [
            '-F',
            '/dev/null',
            '-i',
            str(scratch / 'key'),
            '-o',
            'IdentitiesOnly=yes',
            '-o',
            'BatchMode=yes',
            '-o',
            'ConnectTimeout=3',
            '-o',
            'StrictHostKeyChecking=accept-new',
            '-o',
            f'UserKnownHostsFile={scratch / "known_hosts"}',
        ]
        self.port = ssh_port

    def command(self, command: str, *, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ['ssh', *self.options, '-p', str(self.port), 'tester@127.0.0.1', command],
            check=check,
            capture_output=True,
            text=True,
            timeout=15,
        )

    def copy(self, source: Path, destination: str, *, download: bool = False) -> None:
        remote = 'tester@127.0.0.1:' + destination
        paths = [remote, str(source)] if download else [str(source), remote]
        subprocess.run(['scp', *self.options, '-P', str(self.port), *paths], check=True, timeout=120)


@contextmanager
def running_vm(command: list[str], log: BinaryIO) -> Generator[subprocess.Popen[bytes]]:
    process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
    try:
        yield process
    finally:
        stop_group(process)


def check_app(guest: Guest) -> None:
    guest.command('test ! -f ~/error.txt || { cat ~/error.txt; exit 1; }; test -f ~/ready.json')


def wait_guest(guest: Guest, process: subprocess.Popen[bytes], deadline: float, command: str) -> None:
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise ValueError('QEMU exited; inspect qemu.log and serial.log')
        try:
            if guest.command(command, check=False).returncode == 0:
                return
        except subprocess.TimeoutExpired:
            pass
        time.sleep(1)
    raise TimeoutError('Guest readiness timed out; inspect the retained logs')


def confirm(description: str, deadline: float, process: subprocess.Popen[bytes]) -> None:
    print(description + '\nPass? [yes/no] ', end='', flush=True)
    while time.monotonic() < deadline and process.poll() is None:
        readable, _, _ = select.select([sys.stdin], [], [], min(1, max(0, deadline - time.monotonic())))
        if readable:
            if sys.stdin.readline().strip().lower() not in {'yes', 'y'}:
                raise ValueError('Device acceptance was not confirmed; case remains incomplete')
            return
    raise TimeoutError('Device acceptance timed out or QEMU exited')


def run(options: Options, output: TerminalOutput) -> None:
    vendor, product = usb_identity(options.usb)
    if options.timeout < 60 or not sys.stdin.isatty():
        raise ValueError('Device acceptance requires an interactive terminal and a timeout of at least 60 seconds')
    if not options.image.is_file() or not options.firmware.is_file():
        raise ValueError('The pinned guest image and ARM64 firmware must both exist')
    machine = machine_arguments(platform.system(), platform.machine(), options.machine, options.firmware)
    capability = subprocess.run([machine[0], '-device', 'help'], check=True, capture_output=True, text=True).stdout
    if '"usb-host"' not in capability:
        raise ValueError('This QEMU build lacks host USB passthrough support')
    evidence = options.output.resolve() / uuid.uuid4().hex
    evidence.mkdir(parents=True)
    output.panel(str(evidence), title='Evidence directory')
    output.panel('Building the ARM64 production AppImage through Nix.', title='Desktop VM acceptance')
    app = build(options.appimage, out_link=evidence / 'appimage')
    with app.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    report: Object = {
        'status': 'incomplete',
        'appimage': str(app),
        'sha256': digest,
        'usb': options.usb,
        'guest_image': str(options.image),
        'ubuntu': '26.04',
        'image_build': '20261016',
        'package_snapshot': '20261017T120000Z',
        'machine': machine,
        'manual_steps': [],
    }
    completed: list[str] = []
    report['manual_steps'] = completed
    connection: Qmp | None = None
    guest: Guest | None = None
    previous = signal.signal(signal.SIGTERM, terminated)
    try:
        with tempfile.TemporaryDirectory(prefix='garmin-vm-', dir='/tmp') as temporary:
            scratch = Path(temporary)
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(scratch / 'key')], check=True)
            config = seed((scratch / 'key.pub').read_text())
            (scratch / 'user-data').write_text('#cloud-config\n' + yaml.safe_dump(config, sort_keys=False))
            (scratch / 'meta-data').write_text('instance-id: ' + uuid.uuid4().hex + '\n')
            subprocess.run(
                [
                    'xorriso',
                    '-as',
                    'mkisofs',
                    '-quiet',
                    '-volid',
                    'cidata',
                    '-joliet',
                    '-rock',
                    '-output',
                    str(scratch / 'seed.iso'),
                    str(scratch / 'user-data'),
                    str(scratch / 'meta-data'),
                ],
                check=True,
            )
            subprocess.run(
                [
                    'qemu-img',
                    'create',
                    '-f',
                    'qcow2',
                    '-F',
                    'qcow2',
                    '-b',
                    str(options.image),
                    str(scratch / 'disk.qcow2'),
                    '24G',
                ],
                check=True,
            )
            ssh_port, vnc_port = port(), port()
            guest = Guest(ssh_port, scratch)
            command = [
                *machine,
                '-drive',
                f'file={scratch / "disk.qcow2"},if=virtio,format=qcow2',
                '-drive',
                f'file={scratch / "seed.iso"},if=none,id=seed,format=raw,readonly=on',
                '-device',
                'virtio-blk-pci,drive=seed',
                '-device',
                'virtio-gpu-pci,xres=1280,yres=960',
                '-device',
                'qemu-xhci',
                '-device',
                'usb-kbd',
                '-device',
                'usb-tablet',
                '-netdev',
                f'user,id=network,hostfwd=tcp:127.0.0.1:{ssh_port}-:22',
                '-device',
                'virtio-net-pci,netdev=network',
                '-display',
                'none',
                '-vnc',
                f'127.0.0.1:{vnc_port - 5900}',
                '-serial',
                f'file:{evidence / "serial.log"}',
                '-qmp',
                f'unix:{scratch / "qmp.sock"},server=on,wait=off',
            ]
            deadline = time.monotonic() + options.timeout
            with (evidence / 'qemu.log').open('wb') as log, running_vm(command, log) as qemu:
                try:
                    output.panel('Booting Ubuntu and installing the pinned desktop packages.', title='Preparing guest')
                    wait_guest(guest, qemu, deadline, 'test -f /var/lib/cloud/instance/boot-finished')
                    guest.command('sudo cloud-init status --wait')
                    viewer = f'vnc://127.0.0.1:{vnc_port}'
                    output.panel(viewer, title='Open the guest desktop')
                    if platform.system() == 'Darwin':
                        subprocess.run(['open', viewer], check=True)
                    connection = Qmp(scratch / 'qmp.sock')
                    connection.attach(vendor, product)
                    wait_guest(
                        guest,
                        qemu,
                        min(deadline, time.monotonic() + 30),
                        f'test "$(lsusb -d {vendor:04x}:{product:04x} | wc -l)" -eq 1',
                    )
                    guest.copy(app, '/home/tester/app.pending')
                    guest.command('mv /home/tester/app.pending /home/tester/garmin-toolkit.AppImage')
                    (scratch / 'input.json').write_text(json.dumps({'sha256': digest}) + '\n')
                    guest.copy(scratch / 'input.json', '/home/tester/input.pending')
                    guest.command('mv /home/tester/input.pending /home/tester/input.json')
                    wait_guest(
                        guest, qemu, min(deadline, time.monotonic() + 180), 'test -f ~/ready.json -o -f ~/error.txt'
                    )
                    guest.command('test ! -f ~/error.txt || { cat ~/error.txt; exit 1; }')
                    guest.copy(evidence / 'startup.json', '/home/tester/ready.json', download=True)
                    guest.copy(evidence / 'packages.txt', '/home/tester/packages.txt', download=True)
                    report['startup'] = object_value(json.loads((evidence / 'startup.json').read_text()))
                    connection.screenshot(evidence / 'startup.png')
                    for name, description in CASE:
                        confirm(description, deadline, qemu)
                        check_app(guest)
                        connection.screenshot(evidence / f'{name}.png')
                        completed.append(name)
                    guest.copy(evidence / 'app.log', '/home/tester/app.log', download=True)
                    guest.command(
                        'kill -0 "$(python3 -c \'import json; print(json.load(open("ready.json"))["pid"])\')"'
                    )
                finally:
                    try:
                        with suppress(subprocess.SubprocessError, OSError):
                            diagnostic = guest.command(
                                'sudo tail -n 200 /var/log/cloud-init-output.log; '
                                'sudo journalctl -u lightdm -b -n 100 --no-pager',
                                check=False,
                            )
                            (evidence / 'guest.log').write_text(diagnostic.stdout + diagnostic.stderr)
                        for name in ['app.log', 'error.txt']:
                            with suppress(subprocess.SubprocessError, OSError):
                                if guest.command('test -f /home/tester/' + name, check=False).returncode == 0:
                                    guest.copy(evidence / name, '/home/tester/' + name, download=True)
                    finally:
                        if connection is not None:
                            connection.close()
        report['status'] = 'passed'
    except BaseException as error:
        report['error'] = str(error) or type(error).__name__
        raise
    finally:
        signal.signal(signal.SIGTERM, previous)
        (evidence / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    output.panel('All startup and device acceptance checks passed.', title='Desktop VM acceptance')


def main() -> None:
    output = TerminalOutput()
    try:
        run(tyro.cli(Options), output)
    except KeyboardInterrupt:
        output.panel('Interrupted.', title='Desktop VM acceptance incomplete', error=True)
        sys.exit(130)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        if isinstance(error, subprocess.CalledProcessError):
            if error.stdout:
                output.panel(str(error.stdout), title='Command output', error=True)
            if error.stderr:
                output.panel(str(error.stderr), title='Command error', error=True)
        output.panel(str(error), title='Desktop VM acceptance incomplete', style='red', error=True)
        sys.exit(1)


if __name__ == '__main__':
    main()
