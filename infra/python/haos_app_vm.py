"""Exercise a locally built Home Assistant app in disposable HAOS."""

import base64
import hashlib
import ipaddress
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from collections.abc import Callable
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path
from typing import cast
from urllib.parse import urlsplit

import tyro
from rich.text import Text

from cli_output import TerminalOutput
from haos_platform import (
    Architecture,
    Platform,
    inspect_archive,
    resolve,
    unpack_archive,
    validate_archive,
)
from haos_repository import serve_repository
from haos_runtime import SCRATCH_ROOT, download_image, prepare_image, qemu_command, stop, verify_scope
from hass_release_contract import check_shape
from json_data import (
    Object,
    file_sha256,
    is_object,
    json_bytes,
    object_rows,
    object_value,
    strings,
    text,
    unique_object,
)
from processes import ACCEPTANCE_SCOPE, stop_group, vm_scope

OUTPUT = TerminalOutput()
MAX_OVERLAY_BYTES = 8 * 1024**3
APP_CONFIG = Path(__file__).resolve().parents[2] / 'apps/garmin-hass/addon/config.yaml'
GUEST_APP_DIR = '/mnt/data/supervisor/apps/local/garmin_toolkit'
PUBLISHED_IMAGE = 'ghcr.io/kubijo/garmin-toolkit-hass'
TEST_IMAGE = 'localhost:5000/garmin-toolkit-hass'


@dataclass(frozen=True)
class Options:
    """Load a local app archive into an isolated HAOS guest."""

    app_archive: Path
    """Nix-built garmin-hass-app-image OCI archive."""

    arch: Architecture = 'auto'
    """Guest CPU architecture; auto matches the host."""

    image: Path | None = None
    """Pinned compressed HAOS OVA image; download into temporary storage when omitted."""

    timeout: int = 900
    """Maximum time to reach guest SSH, in seconds."""

    repository: Path | None = None
    """Generated store tree; serve a disposable Git candidate through the guest's loopback registry."""

    repository_url: str | None = None
    """Existing HTTP(S) candidate repository URL, including its branch suffix."""

    registry_digest: str | None = None
    """Independently expected registry pull digest; required with repository-url."""

    registry_image: str = PUBLISHED_IMAGE
    """Generic image repository named by the remote candidate's app metadata."""

    registry_tag: str | None = None
    """Staging tag used by remote metadata; the native archive retains its release version."""

    baseline_archive: tyro.conf.Suppress[Path | None] = None
    candidate_version: tyro.conf.Suppress[str | None] = None


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


def forward_port(key: Path, ssh_port: int, local_port: int, remote_port: int, log: Path) -> subprocess.Popen[str]:
    command = ssh_command(key, ssh_port)
    command[1:1] = [
        '-N',
        '-o',
        'ExitOnForwardFailure=yes',
        '-o',
        'ServerAliveInterval=15',
        '-o',
        'ServerAliveCountMax=2',
        '-L',
        f'127.0.0.1:{local_port}:127.0.0.1:{remote_port}',
    ]
    with log.open('w') as output:
        return subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, text=True, start_new_session=True)


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


def assert_started(key: Path, port: int, slug: str = 'local_garmin_toolkit') -> None:
    app = guest_data(key, port, f'ha apps info {slug}')
    if app.get('state') != 'started' or app.get('ingress') is not True:
        raise RuntimeError(f'The installed app is not started with ingress: {app!r}')


def reload_store(
    key: Path, port: int, slug: str = 'local_garmin_toolkit', *, present: bool = True, version: str | None = None
) -> None:
    if version is not None and not present:
        raise ValueError('A Store version expectation requires a visible app')
    for attempt in range(3):
        try:
            run(ssh_command(key, port, 'ha store reload'), timeout=90)
        except RuntimeError as error:
            if 'context deadline exceeded' not in str(error):
                raise
            print('Supervisor store refresh is still pending; checking local app discovery...', flush=True)
        info = guest_data(key, port, 'ha store info')
        app = next((app for app in object_rows(info.get('apps', info.get('addons'))) if app.get('slug') == slug), None)
        if (app is not None) == present and (
            version is None or app is not None and app.get('version_latest') == version
        ):
            print(f'Supervisor store {slug}: {"present" if present else "withdrawn"}.', flush=True)
            return
        if app is not None and version is not None:
            print(
                f'Supervisor store {slug}: waiting for version {version}; found {app.get("version_latest")!r}.',
                flush=True,
            )
        if attempt < 2:
            time.sleep(5)
    raise RuntimeError(
        'Supervisor store did not reach the expected app visibility or version after bounded refresh retries'
    )


def assert_gateway_response(key: Path, port: int, slug: str = 'local_garmin_toolkit') -> None:
    bindings = run(
        ssh_command(key, port, "docker inspect -f '{{json .HostConfig.PortBindings}}' app_" + slug)
    ).stdout.strip()
    if bindings not in {'null', '{}'}:
        raise RuntimeError(f'The app exposed a host port outside ingress: {bindings}')
    address = run(
        ssh_command(
            key,
            port,
            "docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' app_" + slug,
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


def assert_non_gateway_rejected(key: Path, port: int, slug: str = 'local_garmin_toolkit') -> None:
    address = run(
        ssh_command(
            key,
            port,
            "docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' app_" + slug,
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


def assert_supervisor_backup_restore(key: Path, port: int, app_slug: str = 'local_garmin_toolkit') -> None:
    data_dir = f'/mnt/data/supervisor/apps/data/{app_slug}'
    generation = str(uuid.uuid4())
    generation_dir = f'{data_dir}/storage-generations/{generation}'
    run(ssh_command(key, port, f'ha apps stop {app_slug}'), timeout=90)
    run(ssh_command(key, port, f'mkdir -p {generation_dir}'))
    run(ssh_command(key, port, f'mv {data_dir}/storage.sqlite3 {data_dir}/storage.sqlite3.original'))
    run(ssh_command(key, port, f'cp {data_dir}/storage.sqlite3.original {generation_dir}/storage.sqlite3'))
    selection = json.dumps({'active': {'Generation': generation}, 'rollback': 'Original'})
    send_text(key, port, selection, f'{data_dir}/storage-selection.json')
    run(ssh_command(key, port, f'ha apps start {app_slug}'), timeout=120)
    assert_started(key, port, app_slug)
    assert_gateway_response(key, port, app_slug)
    run(ssh_command(key, port, f'mv {data_dir}/storage.sqlite3.original {data_dir}/storage.sqlite3'))
    backup = guest_data(key, port, f'ha backups new --app {app_slug} --no-progress', timeout=600)
    slug = backup.get('slug')
    if not isinstance(slug, str) or not slug.isalnum():
        raise RuntimeError(f'Supervisor returned an invalid backup slug: {backup!r}')
    print(f'Supervisor cold backup created: {slug}', flush=True)
    assert_started(key, port, app_slug)
    run(ssh_command(key, port, f'ha apps stop {app_slug}'), timeout=90)
    send_text(
        key,
        port,
        json.dumps({'active': 'Original', 'rollback': None}),
        f'{data_dir}/storage-selection.json',
    )
    run(ssh_command(key, port, f'rm -r {generation_dir}'))
    run(ssh_command(key, port, f'ha apps start {app_slug}'), timeout=120)
    assert_started(key, port, app_slug)
    restored_response: object = json.loads(
        run(
            ssh_command(key, port, f'ha backups restore {slug} --app {app_slug} --no-progress --raw-json'),
            timeout=600,
        ).stdout
    )
    if not isinstance(restored_response, dict) or cast('dict[str, object]', restored_response).get('result') != 'ok':
        raise RuntimeError(f'Supervisor restore failed: {restored_response!r}')
    assert_started(key, port, app_slug)
    restored = run(ssh_command(key, port, f'cat {data_dir}/storage-selection.json')).stdout
    if json.loads(restored).get('active') != {'Generation': generation}:
        raise RuntimeError('Supervisor restore did not recover the selected storage generation')
    run(ssh_command(key, port, f'test -s {generation_dir}/storage.sqlite3'))
    assert_gateway_response(key, port, app_slug)
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


def reboot_guest(
    process: subprocess.Popen[str], runtime: Path, key: Path, port: int, *, timeout: int, overlay_limit: int
) -> dict[str, str]:
    command = ssh_command(key, port, 'cat /proc/sys/kernel/random/boot_id')
    before = str(uuid.UUID(run(command).stdout.strip()))
    print('Rebooting disposable HAOS and waiting for a new kernel boot ID...', flush=True)
    result = subprocess.run(
        ssh_command(key, port, 'ha host reboot'), capture_output=True, text=True, timeout=30, check=False
    )
    if result.returncode not in (0, 255):
        raise RuntimeError(f'HAOS reboot request failed: {result.stderr[-1000:]}')
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('HAOS VM exited during reboot')
        if (runtime / 'overlay.qcow2').stat().st_size > overlay_limit:
            raise RuntimeError('HAOS VM overlay exceeded its configured file-size limit')
        try:
            probe = subprocess.run(command, capture_output=True, text=True, timeout=8, check=False)
        except subprocess.TimeoutExpired:
            pass
        else:
            if probe.returncode == 0:
                after = str(uuid.UUID(probe.stdout.strip()))
                if after != before:
                    wait_for_supervisor(
                        process,
                        runtime / 'overlay.qcow2',
                        key,
                        port,
                        max(1, int(deadline - time.monotonic())),
                        overlay_limit=overlay_limit,
                    )
                    print(f'HAOS kernel boot ID changed: {before} -> {after}', flush=True)
                    return {'before': before, 'after': after}
        time.sleep(3)
    raise TimeoutError('HAOS kernel boot ID did not change within the reboot window')


def registry_manifest(key: Path, port: int, repository: str, digest: str, config_digest: str, architecture: str) -> str:
    # Docker's verbose output retains the original manifest bytes as base64 Raw.
    # Address the registry by the independently expected digest, never by its mutable tag.
    insecure = ' --insecure' if repository.startswith('localhost:5000/') else ''
    document: object = json.loads(
        run(
            ssh_command(key, port, f'docker manifest inspect --verbose{insecure} {repository}@{digest}'), timeout=90
        ).stdout,
        object_pairs_hook=unique_object,
    )
    manifests = [object_value(document)] if is_object(document) else object_rows(document)
    selected: list[Object] = []
    for manifest in manifests:
        platform = object_value(manifest.get('Descriptor')).get('platform')
        if is_object(platform) and platform.get('architecture') == architecture and platform.get('os') == 'linux':
            selected.append(manifest)
    if len(selected) != 1:
        raise RuntimeError('Registry must provide exactly one manifest for the expected platform')
    manifest = selected[0]
    raw = base64.b64decode(text(manifest.get('Raw')), validate=True)
    actual = 'sha256:' + hashlib.sha256(raw).hexdigest()
    if actual != object_value(manifest.get('Descriptor')).get('digest'):
        raise RuntimeError('Registry manifest bytes differ from their descriptor digest')
    if not isinstance(document, list) and actual != digest:
        raise RuntimeError('Registry manifest differs from the independently expected digest')
    if (
        object_value(object_value(json.loads(raw, object_pairs_hook=unique_object)).get('config')).get('digest')
        != config_digest
    ):
        raise RuntimeError('Registry manifest config differs from the expected archive configuration')
    return actual


def assert_installed_image(
    key: Path, port: int, slug: str, *, reference: str, digest: str, config_digest: str, architecture: str
) -> Object:
    repository = reference.rsplit(':', 1)[0]
    manifest = registry_manifest(key, port, repository, digest, config_digest, architecture)
    container = object_value(
        json.loads(run(ssh_command(key, port, "docker inspect -f '{{json .}}' app_" + slug)).stdout)
    )
    actual_image = text(container.get('Image'))
    actual_reference = object_value(container.get('Config')).get('Image')
    image = object_value(
        json.loads(run(ssh_command(key, port, "docker image inspect -f '{{json .}}' " + actual_image)).stdout)
    )
    # Older engines identify images by config; containerd engines use a manifest/index.
    selected = container.get('ImageManifestDescriptor')
    index_matches = actual_image == digest and is_object(selected) and selected.get('digest') == manifest
    if (
        (actual_image not in (config_digest, manifest) and not index_matches)
        or image.get('Id') != actual_image
        or actual_reference != reference
    ):
        raise RuntimeError('Installed app image differs from the expected archive/configuration')
    if image.get('Architecture') != architecture or image.get('Os') != 'linux':
        raise RuntimeError('Installed app platform differs from the expected archive')
    if f'{repository}@{digest}' not in strings(image.get('RepoDigests')):
        raise RuntimeError('Installed app registry digest differs from the independently expected digest')
    return {
        'app_slug': slug,
        'image_reference': reference,
        'registry_digest': digest,
        'manifest_digest': manifest,
        'config_digest': config_digest,
        'architecture': architecture,
    }


def publish_image(
    archive: Path, version: str, key: Path, port: int, runtime: Path, *, registry_tag: str | None = None
) -> str:
    destination = registry_tag or version
    if not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}', destination):
        raise ValueError('Invalid disposable image tag')
    print(f'Publishing {destination} to the disposable guest registry...', flush=True)
    inspected = inspect_archive(archive)
    if inspected.repository_tags != (f'{PUBLISHED_IMAGE}:{version}',):
        raise ValueError('Archive does not contain the canonical app image and version')
    with tempfile.TemporaryDirectory(prefix='image-import.', dir=runtime) as temporary:
        layout = Path(temporary) / 'oci'
        unpack_archive(archive, layout)
        local_port = free_port()
        tunnel = forward_port(key, port, local_port, 5000, Path(temporary) / 'tunnel.log')
        try:
            deadline = time.monotonic() + 10
            while True:
                if tunnel.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError('Disposable registry SSH tunnel did not start')
                with socket.socket() as connection:
                    if connection.connect_ex(('127.0.0.1', local_port)) == 0:
                        break
                time.sleep(0.05)
            run(
                [
                    'skopeo',
                    '--insecure-policy',
                    '--tmpdir',
                    temporary,
                    '--command-timeout',
                    '5m',
                    'copy',
                    '--all',
                    '--preserve-digests',
                    '--image-parallel-copies',
                    '1',
                    '--retry-times',
                    '2',
                    '--dest-no-creds',
                    '--dest-tls-verify=false',
                    'oci:' + str(layout),
                    f'docker://127.0.0.1:{local_port}/garmin-toolkit-hass:{destination}',
                ],
                timeout=330,
            )
            registry_manifest(
                key,
                port,
                TEST_IMAGE,
                inspected.manifest_digest,
                inspected.config_digest,
                text(inspected.configuration['architecture']),
            )
        finally:
            stop_group(tunnel)
    return inspected.manifest_digest


def install_app(
    options: Options,
    runtime: Path,
    key: Path,
    port: int,
    services: ExitStack,
) -> str:
    config_text, version = test_config()
    candidate_version = options.candidate_version or options.registry_tag or version
    inspected = inspect_archive(options.app_archive)
    guest = resolve(options.arch)
    # This registry is confined to the disposable guest and also supplies the untrusted ingress probe.
    run(
        ssh_command(
            key,
            port,
            'docker run -d --restart=unless-stopped --name garmin-test-registry -p 127.0.0.1:5000:5000 registry:2',
        ),
        timeout=180,
    )
    image = options.registry_image if options.repository_url else TEST_IMAGE
    expected: Object = {
        'image_reference': f'{image}:{options.registry_tag or candidate_version}',
        'registry_digest': options.registry_digest,
        'config_digest': inspected.config_digest,
        'manifest_digest': inspected.manifest_digest,
        'architecture': text(inspected.configuration.get('architecture')),
        'version': candidate_version,
        'archive_version': version,
        'archive_sha256': file_sha256(options.app_archive),
    }
    initial_version = candidate_version
    if not options.repository_url:
        expected['registry_digest'] = publish_image(
            options.app_archive, version, key, port, runtime, registry_tag=candidate_version
        )
        if options.baseline_archive is not None:
            baseline = inspect_archive(options.baseline_archive)
            baseline_version = text(
                object_value(object_value(baseline.configuration['config'])['Labels'])['io.hass.version']
            )
            validate_archive(options.baseline_archive, guest, baseline_version, inspected=baseline)
            if baseline_version == candidate_version:
                raise ValueError('Lifecycle baseline and candidate require distinct Supervisor versions')
            initial_version = baseline_version
            previous: Object = {
                'image_reference': f'{image}:{baseline_version}',
                'registry_digest': publish_image(options.baseline_archive, baseline_version, key, port, runtime),
                'config_digest': baseline.config_digest,
                'manifest_digest': baseline.manifest_digest,
                'architecture': text(baseline.configuration['architecture']),
                'version': baseline_version,
                'archive_version': baseline_version,
                'archive_sha256': file_sha256(options.baseline_archive),
            }
            (runtime / 'lifecycle-images.json').write_bytes(json_bytes({'baseline': previous, 'candidate': expected}))
            expected = previous
    repository_url = options.repository_url
    if options.repository is not None:
        repository_url = services.enter_context(
            serve_repository(
                options.repository,
                runtime,
                image=image,
                architecture=guest.architecture,
                version=version,
                initial_version=initial_version,
            )
        )
    if repository_url is not None:
        run(ssh_command(key, port, 'ha store add ' + shlex.quote(repository_url)), timeout=180)
        slug = hashlib.sha1(repository_url.lower().encode()).hexdigest()[:8] + '_garmin_toolkit'
    else:
        slug = 'local_garmin_toolkit'
        run(ssh_command(key, port, f'mkdir -p {GUEST_APP_DIR}'))
        send_text(key, port, config_text, f'{GUEST_APP_DIR}/config.yaml')
    reload_store(key, port, slug)
    print(f'Installing and starting {slug}...', flush=True)
    run(ssh_command(key, port, f'ha apps install {slug}'), timeout=600)
    run(ssh_command(key, port, f'ha apps start {slug}'), timeout=120)
    if expected['registry_digest'] is None:
        raise ValueError('Repository installation requires an expected registry digest')
    identity = assert_installed_image(
        key,
        port,
        slug,
        reference=text(expected['image_reference']),
        digest=text(expected['registry_digest']),
        config_digest=text(expected['config_digest']),
        architecture=text(expected['architecture']),
    )
    if identity['manifest_digest'] != expected['manifest_digest']:
        raise RuntimeError('Installed app manifest differs from the native OCI archive')
    installed_version = guest_data(key, port, f'ha apps info {slug}').get('version')
    if installed_version != initial_version:
        raise RuntimeError('Supervisor did not install the expected app version')
    identity.update(
        repository_url=repository_url,
        version=installed_version,
        archive_version=expected['archive_version'],
        archive_sha256=expected['archive_sha256'],
    )
    (runtime / 'installation.json').write_bytes(json_bytes(identity))
    return slug


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
    overlay = runtime / 'overlay.qcow2'
    run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image), str(overlay)])
    if guest.firmware_vars is not None:
        shutil.copyfile(guest.firmware_vars, runtime / 'ovmf-vars.fd')
    config, key = config_drive(runtime)
    port = free_port()
    owner = os.environ.get(ACCEPTANCE_SCOPE)
    unit = vm_scope(owner).removesuffix('.scope') if owner else f'garmin-haos-app-{uuid.uuid4().hex[:12]}'
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
    services = ExitStack()
    slug = 'local_garmin_toolkit'
    try:
        if on_boot is not None:
            on_boot(unit, port)
        verify_scope(unit, min(time.monotonic() + 15, time.monotonic() + options.timeout))
        wait_for_guest(process, overlay, key, port, runtime, options.timeout, overlay_limit=overlay_limit)
        print(
            wait_for_supervisor(process, overlay, key, port, options.timeout, overlay_limit=overlay_limit), flush=True
        )
        print(run(ssh_command(key, port, 'df -h /mnt/data; free -h')).stdout, flush=True)
        slug = install_app(options, runtime, key, port, services)
        assert_started(key, port, slug)
        assert_gateway_response(key, port, slug)
        assert_non_gateway_rejected(key, port, slug)
        if on_started is None:
            assert_supervisor_backup_restore(key, port, slug)
        else:
            on_started(process, key, port, slug)
    except OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError:
        print(f'VM exit code: {process.poll()}', flush=True)
        if process.poll() is None:
            for command in (
                'df -h /mnt/data; docker ps --format "{{.Names}} {{.Status}}"',
                'docker logs hassio_supervisor --tail 80',
                'ha supervisor logs --no-progress',
                f'ha apps info {slug} --raw-json',
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
        try:
            stop(process, unit)
        finally:
            services.close()


def validate_options(options: Options) -> Platform:
    if options.candidate_version is not None and (
        options.repository is None
        or re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-.][A-Za-z0-9][A-Za-z0-9.-]*)?', options.candidate_version) is None
    ):
        raise ValueError('A private candidate version requires a generated repository and a valid version')
    if options.repository is not None and options.repository_url is not None:
        raise ValueError('Select a generated repository or a remote repository URL, not both')
    if options.repository is not None:
        check_shape(options.repository, 'hass-app-repository.dirspec.toml')
    if options.repository_url is not None:
        url = urlsplit(options.repository_url)
        if (
            url.scheme not in ('http', 'https')
            or not url.hostname
            or not url.fragment
            or url.username
            or url.password
            or url.query
            or any(char.isspace() for char in options.repository_url)
        ):
            raise ValueError('Expected an anonymous HTTP(S) repository URL with a branch suffix')
        if not options.registry_digest or not re.fullmatch(r'sha256:[0-9a-f]{64}', options.registry_digest):
            raise ValueError('Remote repository installation requires an independently expected registry digest')
    elif (
        options.registry_digest is not None
        or options.registry_image != PUBLISHED_IMAGE
        or options.registry_tag is not None
    ):
        raise ValueError('Registry overrides require a remote repository URL')
    if options.registry_tag is not None and not re.fullmatch(
        r'[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}', options.registry_tag
    ):
        raise ValueError('Invalid candidate image tag')
    if not re.fullmatch(r'[a-z0-9][a-z0-9./:_-]*', options.registry_image) or '..' in options.registry_image.split('/'):
        raise ValueError('Invalid candidate image repository')
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
