"""Exercise a locally built Home Assistant app in disposable HAOS."""

import ipaddress
import json
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import tyro
from rich.text import Text

from cli_output import TerminalOutput
from haos_platform import Architecture, Platform, resolve, validate_archive
from haos_runtime import SCRATCH_ROOT, download_image, prepare_image, qemu_command, stop, verify_scope

OUTPUT = TerminalOutput()
MAX_OVERLAY_BYTES = 8 * 1024**3
APP_CONFIG = Path(__file__).resolve().parents[2] / 'apps/garmin-hass/addon/config.yaml'
GUEST_APP_DIR = '/mnt/data/supervisor/apps/local/garmin_toolkit'
GUEST_DATA_DIR = '/mnt/data/supervisor/apps/data/local_garmin_toolkit'
PUBLISHED_IMAGE = 'ghcr.io/kubijo/garmin-toolkit-hass'
TEST_IMAGE = 'localhost:5000/garmin-toolkit-hass'


@dataclass(frozen=True)
class Options:
    """Load a local app archive into an isolated HAOS guest."""

    app_archive: Path
    """Nix-built garmin-hass-app-image tar.gz archive."""

    arch: Architecture = 'auto'
    """Guest CPU architecture; auto matches the host."""

    image: Path | None = None
    """Pinned compressed HAOS OVA image; download into temporary storage when omitted."""

    timeout: int = 900
    """Maximum time to reach guest SSH, in seconds."""


def run(command: list[str], *, timeout: int = 30) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, check=False, capture_output=True, text=True, timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError(f'{command[0]} failed ({result.returncode}): {(result.stderr or result.stdout)[-1_000:]}')
    return result


def config_drive(runtime: Path) -> tuple[Path, Path]:
    key = runtime / 'ssh-key'
    run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(key)])
    drive = runtime / 'config.raw'
    run(['truncate', '-s', '64M', str(drive)])
    run(['parted', '-s', str(drive), 'mklabel', 'msdos', 'mkpart', 'primary', 'fat32', '1MiB', '63MiB'])
    filesystem = f'{drive}@@1048576'
    run(['mformat', '-i', filesystem, '-F', '-v', 'CONFIG', '::'])
    run(['mcopy', '-i', filesystem, str(key.with_suffix('.pub')), '::authorized_keys'])
    return drive, key


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return int(listener.getsockname()[1])


def ssh_command(key: Path, port: int, remote: str | None = None) -> list[str]:
    options = (
        'BatchMode=yes',
        'IdentitiesOnly=yes',
        'IdentityAgent=none',
        'ConnectTimeout=3',
        'StrictHostKeyChecking=no',
        'UserKnownHostsFile=/dev/null',
    )
    command = ['ssh', '-F', '/dev/null', '-i', str(key), '-p', str(port)]
    command += [argument for option in options for argument in ('-o', option)]
    return [*command, 'root@127.0.0.1', *([] if remote is None else [remote])]


def send_file(key: Path, port: int, source: Path, destination: str) -> None:
    with source.open('rb') as contents:
        result = subprocess.run(
            ssh_command(key, port, f'cat > {destination}'),
            stdin=contents,
            capture_output=True,
            timeout=300,
            check=False,
        )
    if result.returncode != 0:
        raise RuntimeError(
            f'Could not send {source.name} to the guest: {result.stderr.decode(errors="replace")[-1_000:]}'
        )


def send_text(key: Path, port: int, contents: str, destination: str) -> None:
    result = subprocess.run(
        ssh_command(key, port, f'cat > {destination}'),
        input=contents,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f'Could not send {destination} to the guest: {result.stderr[-1_000:]}')


def test_config() -> tuple[str, str]:
    original = APP_CONFIG.read_text()
    image_line = f'image: {PUBLISHED_IMAGE}'
    if original.count(image_line) != 1:
        raise ValueError('App metadata must contain exactly one published image reference')
    versions = [
        line.removeprefix('version: ').strip() for line in original.splitlines() if line.startswith('version: ')
    ]
    if len(versions) != 1 or not versions[0]:
        raise ValueError('App metadata must contain exactly one version')
    return original.replace(image_line, f'image: {TEST_IMAGE}'), versions[0]


def guest_data(key: Path, port: int, command: str, *, timeout: int = 90) -> dict[str, object]:
    response: object = json.loads(run(ssh_command(key, port, f'{command} --raw-json'), timeout=timeout).stdout)
    if not isinstance(response, dict):
        raise RuntimeError(f'Unexpected Home Assistant response for {command}: {response!r}')
    payload = cast('dict[str, object]', response)
    if payload.get('result') != 'ok':
        raise RuntimeError(f'Unexpected Home Assistant response for {command}: {response!r}')
    data = payload.get('data')
    if not isinstance(data, dict):
        raise RuntimeError(f'Home Assistant returned no data for {command}: {response!r}')
    return cast('dict[str, object]', data)


def assert_started(key: Path, port: int) -> None:
    app = guest_data(key, port, 'ha apps info local_garmin_toolkit')
    if app.get('state') != 'started' or app.get('ingress') is not True:
        raise RuntimeError(f'The installed app is not started with ingress: {app!r}')


def reload_store(key: Path, port: int) -> None:
    for attempt in range(3):
        try:
            run(ssh_command(key, port, 'ha store reload'), timeout=90)
        except RuntimeError as error:
            if 'context deadline exceeded' not in str(error):
                raise
            print('Supervisor store refresh is still pending; checking local app discovery...', flush=True)
        info = run(ssh_command(key, port, 'ha store info --raw-json'), timeout=90).stdout
        if '"slug":"local_garmin_toolkit"' in info:
            print('Supervisor discovered local_garmin_toolkit.', flush=True)
            return
        if attempt < 2:
            time.sleep(5)
    raise RuntimeError('Supervisor store did not discover the local app after bounded refresh retries')


def assert_gateway_response(key: Path, port: int) -> None:
    bindings = run(
        ssh_command(key, port, "docker inspect -f '{{json .HostConfig.PortBindings}}' app_local_garmin_toolkit")
    ).stdout.strip()
    if bindings not in {'null', '{}'}:
        raise RuntimeError(f'The app exposed a host port outside ingress: {bindings}')
    address = run(
        ssh_command(
            key,
            port,
            "docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' app_local_garmin_toolkit",
        )
    ).stdout.strip()
    ipaddress.IPv4Address(address)
    probe = (
        'import urllib.request; '
        f'response = urllib.request.urlopen("http://{address}:8099/", timeout=10); '
        'body = response.read(); '
        'assert response.status == 200 and len(body) > 100; '
        'print(f"Trusted gateway returned {response.status} and {len(body)} bytes")'
    )
    result = run(ssh_command(key, port, f'docker exec hassio_supervisor python3 -c {shlex.quote(probe)}'))
    print(result.stdout.strip(), flush=True)


def assert_non_gateway_rejected(key: Path, port: int) -> None:
    address = run(
        ssh_command(
            key,
            port,
            "docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' app_local_garmin_toolkit",
        )
    ).stdout.strip()
    ipaddress.IPv4Address(address)
    run(ssh_command(key, port, 'docker network connect hassio garmin-test-registry'))
    probe = subprocess.run(
        ssh_command(key, port, f'docker exec garmin-test-registry wget -S -O /dev/null http://{address}:8099/'),
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if probe.returncode == 0 or '403 Forbidden' not in probe.stdout + probe.stderr:
        raise RuntimeError(f'Non-gateway app request did not return HTTP 403: {(probe.stdout + probe.stderr)[-800:]}')
    print('Non-gateway request returned HTTP 403; no host port is published.', flush=True)


def assert_supervisor_backup_restore(key: Path, port: int) -> None:
    generation = str(uuid.uuid4())
    generation_dir = f'{GUEST_DATA_DIR}/storage-generations/{generation}'
    run(ssh_command(key, port, 'ha apps stop local_garmin_toolkit'), timeout=90)
    run(ssh_command(key, port, f'mkdir -p {generation_dir}'))
    run(ssh_command(key, port, f'mv {GUEST_DATA_DIR}/storage.sqlite3 {GUEST_DATA_DIR}/storage.sqlite3.original'))
    run(ssh_command(key, port, f'cp {GUEST_DATA_DIR}/storage.sqlite3.original {generation_dir}/storage.sqlite3'))
    selection = json.dumps({'active': {'Generation': generation}, 'rollback': 'Original'})
    send_text(key, port, selection, f'{GUEST_DATA_DIR}/storage-selection.json')
    run(ssh_command(key, port, 'ha apps start local_garmin_toolkit'), timeout=120)
    assert_started(key, port)
    assert_gateway_response(key, port)
    run(ssh_command(key, port, f'mv {GUEST_DATA_DIR}/storage.sqlite3.original {GUEST_DATA_DIR}/storage.sqlite3'))
    backup = guest_data(key, port, 'ha backups new --app local_garmin_toolkit --no-progress', timeout=600)
    slug = backup.get('slug')
    if not isinstance(slug, str) or not slug.isalnum():
        raise RuntimeError(f'Supervisor returned an invalid backup slug: {backup!r}')
    print(f'Supervisor cold backup created: {slug}', flush=True)
    assert_started(key, port)
    run(ssh_command(key, port, 'ha apps stop local_garmin_toolkit'), timeout=90)
    send_text(
        key,
        port,
        json.dumps({'active': 'Original', 'rollback': None}),
        f'{GUEST_DATA_DIR}/storage-selection.json',
    )
    run(ssh_command(key, port, f'rm -r {generation_dir}'))
    run(ssh_command(key, port, 'ha apps start local_garmin_toolkit'), timeout=120)
    assert_started(key, port)
    restored_response: object = json.loads(
        run(
            ssh_command(key, port, f'ha backups restore {slug} --app local_garmin_toolkit --no-progress --raw-json'),
            timeout=600,
        ).stdout
    )
    if not isinstance(restored_response, dict) or cast('dict[str, object]', restored_response).get('result') != 'ok':
        raise RuntimeError(f'Supervisor restore failed: {restored_response!r}')
    assert_started(key, port)
    restored = run(ssh_command(key, port, f'cat {GUEST_DATA_DIR}/storage-selection.json')).stdout
    if json.loads(restored).get('active') != {'Generation': generation}:
        raise RuntimeError('Supervisor restore did not recover the selected storage generation')
    run(ssh_command(key, port, f'test -s {generation_dir}/storage.sqlite3'))
    assert_gateway_response(key, port)
    print('Cold backup restored the selected generation and the app reopened it.', flush=True)


def wait_for_guest(
    process: subprocess.Popen[str],
    image: Path,
    key: Path,
    port: int,
    runtime: Path,
    timeout: int,
    *,
    overlay_limit: int = MAX_OVERLAY_BYTES,
) -> None:
    deadline = time.monotonic() + timeout
    next_notice = time.monotonic()
    while time.monotonic() < deadline:
        if process.poll() is not None:
            output = process.stdout.read() if process.stdout else ''
            raise RuntimeError(f'HAOS VM exited before SSH was ready: {output.strip()}')
        if image.stat().st_size > overlay_limit:
            raise RuntimeError('HAOS VM overlay exceeded its configured file-size limit')
        try:
            probe = subprocess.run(
                ssh_command(key, port, 'true'), capture_output=True, text=True, timeout=8, check=False
            )
        except subprocess.TimeoutExpired:
            continue
        if probe.returncode == 0:
            return
        if time.monotonic() >= next_notice:
            print('Waiting for disposable HAOS guest SSH...', flush=True)
            next_notice = time.monotonic() + 30
        time.sleep(3)
    serial = runtime / 'serial.log'
    tail = serial.read_text(errors='replace')[-2_000:] if serial.exists() else ''
    raise TimeoutError(f'HAOS guest SSH did not become ready in {timeout}s. Serial tail:\n{tail}')


def wait_for_supervisor(
    process: subprocess.Popen[str],
    image: Path,
    key: Path,
    port: int,
    timeout: int,
    *,
    overlay_limit: int = MAX_OVERLAY_BYTES,
) -> str:
    deadline = time.monotonic() + timeout
    next_notice = time.monotonic()
    last_error = ''
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('HAOS VM exited while waiting for Supervisor')
        if image.stat().st_size > overlay_limit:
            raise RuntimeError('HAOS VM overlay exceeded its configured file-size limit')
        try:
            probe = subprocess.run(
                ssh_command(key, port, 'ha info'), capture_output=True, text=True, timeout=15, check=False
            )
        except subprocess.TimeoutExpired:
            last_error = 'HA CLI is still initializing'
        else:
            if probe.returncode == 0 and 'state: running' in probe.stdout:
                return probe.stdout
            details = probe.stderr.strip() or probe.stdout.strip()
            last_error = details.splitlines()[-1] if details else 'HA CLI returned no status'
        if time.monotonic() >= next_notice:
            print(f'Waiting for disposable HAOS Supervisor: {last_error[-180:]}', flush=True)
            next_notice = time.monotonic() + 30
        time.sleep(5)
    raise TimeoutError(f'HAOS Supervisor did not become ready in {timeout}s: {last_error}')


def boot(
    options: Options,
    runtime: Path,
    image: Path,
    *,
    on_started: Callable[[subprocess.Popen[str], Path, int, str], None] | None = None,
    on_boot: Callable[[str, int], None] | None = None,
    lifetime: int | None = None,
    overlay_limit: int = MAX_OVERLAY_BYTES,
) -> None:
    guest = resolve(options.arch)
    config_text, version = test_config()
    overlay = runtime / 'overlay.qcow2'
    run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image), str(overlay)])
    if guest.firmware_vars is not None:
        shutil.copyfile(guest.firmware_vars, runtime / 'ovmf-vars.fd')
    config, key = config_drive(runtime)
    port = free_port()
    unit = f'garmin-haos-app-{uuid.uuid4().hex[:12]}'
    process = subprocess.Popen(
        qemu_command(
            overlay,
            config,
            runtime,
            unit,
            port,
            lifetime or options.timeout,
            guest=guest,
            overlay_limit=overlay_limit,
        ),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    try:
        if on_boot is not None:
            on_boot(unit, port)
        verify_scope(unit, min(time.monotonic() + 15, time.monotonic() + options.timeout))
        wait_for_guest(process, overlay, key, port, runtime, options.timeout, overlay_limit=overlay_limit)
        print(
            wait_for_supervisor(process, overlay, key, port, options.timeout, overlay_limit=overlay_limit), flush=True
        )
        print(run(ssh_command(key, port, 'df -h /mnt/data; free -h')).stdout, flush=True)
        print('Loading the local app image into the disposable guest...', flush=True)
        for attempt in range(3):
            try:
                send_file(key, port, options.app_archive, '/mnt/data/garmin-hass-app.tar.gz')
                break
            except RuntimeError:
                if attempt == 2 or process.poll() is not None:
                    raise
                print('Guest SSH dropped during transfer; waiting for HAOS to settle...', flush=True)
                wait_for_guest(process, overlay, key, port, runtime, 120, overlay_limit=overlay_limit)
                wait_for_supervisor(process, overlay, key, port, 120, overlay_limit=overlay_limit)
        loaded = run(
            ssh_command(
                key, port, 'docker load -i /mnt/data/garmin-hass-app.tar.gz && rm /mnt/data/garmin-hass-app.tar.gz'
            ),
            timeout=600,
        ).stdout
        if f'Loaded image: {PUBLISHED_IMAGE}:{version}' not in loaded:
            raise RuntimeError(f'The archive does not contain the app metadata image and version: {loaded[-500:]}')
        print(loaded, flush=True)
        print('Publishing the image to a registry inside the disposable guest...', flush=True)
        run(
            ssh_command(key, port, 'docker run -d --name garmin-test-registry -p 127.0.0.1:5000:5000 registry:2'),
            timeout=180,
        )
        run(ssh_command(key, port, f'docker tag {PUBLISHED_IMAGE}:{version} {TEST_IMAGE}:{version}'))
        print(run(ssh_command(key, port, f'docker push {TEST_IMAGE}:{version}'), timeout=300).stdout[-800:], flush=True)
        run(ssh_command(key, port, f'mkdir -p {GUEST_APP_DIR}'))
        send_text(key, port, config_text, f'{GUEST_APP_DIR}/config.yaml')
        reload_store(key, port)
        print('Installing local_garmin_toolkit...', flush=True)
        print(run(ssh_command(key, port, 'ha apps install local_garmin_toolkit'), timeout=600).stdout, flush=True)
        print('Starting local_garmin_toolkit...', flush=True)
        print(run(ssh_command(key, port, 'ha apps start local_garmin_toolkit'), timeout=120).stdout, flush=True)
        assert_started(key, port)
        assert_gateway_response(key, port)
        assert_non_gateway_rejected(key, port)
        if on_started is None:
            assert_supervisor_backup_restore(key, port)
        else:
            on_started(process, key, port, unit)
    except OSError, RuntimeError, TimeoutError, subprocess.SubprocessError:
        print(f'VM exit code: {process.poll()}', flush=True)
        if process.poll() is None:
            for command in (
                'df -h /mnt/data; docker ps --format "{{.Names}} {{.Status}}"',
                'docker logs hassio_supervisor --tail 80',
                'ha supervisor logs --no-progress',
                'ha apps info local_garmin_toolkit --raw-json',
            ):
                try:
                    diagnostic = run(ssh_command(key, port, command), timeout=15)
                    print(f'Guest diagnostic:\n{(diagnostic.stdout + diagnostic.stderr)[-6_000:]}', flush=True)
                except OSError, RuntimeError, subprocess.SubprocessError:
                    pass
        serial = runtime / 'serial.log'
        if serial.exists():
            print(f'Serial tail:\n{serial.read_text(errors="replace")[-2_000:]}', flush=True)
        if overlay.exists():
            print(f'Overlay size: {overlay.stat().st_size} bytes', flush=True)
        raise
    finally:
        stop(process, unit)


def validate_options(options: Options) -> Platform:
    guest = resolve(options.arch)
    if sys.platform != 'linux' or not guest.firmware.is_file():
        raise ValueError(f'Linux and {guest.architecture} EFI firmware are required')
    if guest.firmware_vars is not None and not guest.firmware_vars.is_file():
        raise ValueError('x86-64 OVMF variables firmware is missing')
    _, version = test_config()
    validate_archive(options.app_archive, guest, version)
    return guest


def prepare_runtime(options: Options, runtime: Path, guest: Platform) -> Path:
    compressed = options.image
    if compressed is None:
        compressed = runtime / 'haos.qcow2.xz'
        print(f'Downloading pinned {guest.architecture} HAOS 18.3...', flush=True)
        download_image(compressed, guest)
    image = runtime / 'haos.qcow2'
    prepare_image(compressed, image, guest)
    return image


def main(options: Options) -> int:
    if not options.app_archive.is_file() or not 10 <= options.timeout <= 1800:
        OUTPUT.panel(
            Text('Provide a built app archive and a timeout of 10-1800 seconds.'), title='Invalid options', error=True
        )
        return 1
    try:
        guest = validate_options(options)
        SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='haos-app.', dir=SCRATCH_ROOT) as directory:
            runtime = Path(directory)
            image = prepare_runtime(options, runtime, guest)
            boot(options, runtime, image)
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError) as error:
        OUTPUT.panel(Text(str(error)), title='HAOS app VM failed', style='red', error=True)
        return 1
    OUTPUT.panel(Text('Disposable HAOS app install, ingress, and cold backup/restore passed.'), title='App verified')
    return 0


if __name__ == '__main__':
    OUTPUT.install_tracebacks()
    sys.exit(main(tyro.cli(Options)))
