"""Architecture and image checks shared by disposable HAOS app runners."""

import hashlib
import io
import json
import os
import platform
import tarfile
from copy import deepcopy
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from archive_io import MAX_METADATA_BYTES, open_tar
from json_data import Object, json_bytes, object_rows, object_value, strings, unique_object

Architecture = Literal['auto', 'amd64', 'aarch64']
BASELINE_VERSION = '0.0.0'


@dataclass(frozen=True)
class Platform:
    architecture: Literal['amd64', 'aarch64']
    image_name: str
    image_sha256: str
    firmware: Path
    firmware_vars: Path | None

    @property
    def image_url(self) -> str:
        return f'https://github.com/home-assistant/operating-system/releases/download/18.3/{self.image_name}'


PLATFORMS = {
    'amd64': Platform(
        'amd64',
        'haos_ova-18.3.qcow2.xz',
        'fae6a728768cc10aff60d4820bfcd40d64cd77fab82c8bd92af13b3d9d414090',
        Path('/usr/share/OVMF/OVMF_CODE_4M.fd'),
        Path('/usr/share/OVMF/OVMF_VARS_4M.fd'),
    ),
    'aarch64': Platform(
        'aarch64',
        'haos_generic-aarch64-18.3.qcow2.xz',
        'f54b659f12c665038134bdf9d4235903048056257368f82369dc230131f5a675',
        Path('/usr/share/qemu-efi-aarch64/QEMU_EFI.fd'),
        None,
    ),
}


def resolve(architecture: Architecture = 'auto') -> Platform:
    if architecture == 'auto':
        machine = platform.machine().lower()
        native = {'x86_64': 'amd64', 'amd64': 'amd64', 'aarch64': 'aarch64', 'arm64': 'aarch64'}.get(machine)
        if native is None:
            raise ValueError(f'Unsupported host CPU architecture: {machine}')
        return PLATFORMS[native]
    return PLATFORMS[architecture]


def acceleration(guest: Platform) -> str:
    if guest == resolve() and os.access('/dev/kvm', os.R_OK | os.W_OK):
        return 'kvm'
    return 'tcg,thread=multi'


def machine_arguments(guest: Platform, runtime: Path, *, force_tcg: bool = False) -> list[str]:
    accelerator = 'tcg,thread=multi' if force_tcg else acceleration(guest)
    if guest.architecture == 'amd64':
        command = ['qemu-system-x86_64', '-machine', 'q35', '-accel', accelerator]
        command += ['-cpu', 'host' if accelerator == 'kvm' else 'max']
        command += ['-drive', f'if=pflash,format=raw,unit=0,readonly=on,file={guest.firmware}']
        command += ['-drive', f'if=pflash,format=raw,unit=1,file={runtime / "ovmf-vars.fd"}']
    else:
        command = ['qemu-system-aarch64', '-machine', 'virt', '-accel', accelerator]
        command += ['-cpu', 'host' if accelerator == 'kvm' else 'cortex-a57', '-bios', str(guest.firmware)]
        command += ['-device', 'ramfb']
    return command


@dataclass(frozen=True)
class ImageArchive:
    configuration: Object
    config_digest: str
    repository_tags: tuple[str, ...]
    layers: tuple[str, ...] = ()


def inspect_archive(archive: Path) -> ImageArchive:
    """Read bounded regular metadata members without extracting the image to disk."""
    with open_tar(archive) as image:
        metadata: dict[str, bytes] = {}
        total = 0
        for member in image:
            if not member.name.endswith('.json'):
                continue
            if member.name in metadata:
                raise ValueError(f'App archive metadata must occur exactly once: {member.name}')
            total += member.size
            if not member.isfile() or member.size > MAX_METADATA_BYTES or total > 16 * MAX_METADATA_BYTES:
                raise ValueError(f'App archive metadata must be a bounded regular file: {member.name}')
            stream = image.extractfile(member)
            with stream:
                metadata[member.name] = stream.read(MAX_METADATA_BYTES + 1)

        def contents(name: str) -> bytes:
            if name not in metadata:
                raise ValueError(f'App archive metadata must occur exactly once: {name}')
            return metadata[name]

        manifest = object_rows(json.loads(contents('manifest.json'), object_pairs_hook=unique_object))
        if len(manifest) != 1:
            raise ValueError('App archive must contain exactly one image')
        entry = manifest[0]
        config_name = entry.get('Config')
        if not isinstance(config_name, str):
            raise ValueError('App archive manifest has no image configuration')
        if Path(config_name).is_absolute() or '..' in Path(config_name).parts:
            raise ValueError('App archive image configuration path is unsafe')
        config_bytes = contents(config_name)
        config = object_value(json.loads(config_bytes, object_pairs_hook=unique_object))
        tags, layers = strings(entry.get('RepoTags', [])), strings(entry.get('Layers', []))
        if any(Path(layer).is_absolute() or '..' in Path(layer).parts for layer in layers):
            raise ValueError('Invalid app archive layers')
        return ImageArchive(
            config,
            'sha256:' + hashlib.sha256(config_bytes).hexdigest(),
            tuple(tags),
            tuple(layers),
        )


def archive_config(archive: Path) -> dict[str, object]:
    return inspect_archive(archive).configuration


def baseline_fixture(source: Path, destination: Path) -> None:
    """Keep candidate layers unchanged; change only metadata for a private Supervisor upgrade fixture."""
    original = inspect_archive(source)
    config = deepcopy(original.configuration)
    labels = object_value(object_value(config['config'])['Labels'])
    if labels['io.hass.version'] == BASELINE_VERSION:
        raise ValueError('The candidate must differ from the fixture baseline version')
    labels['io.hass.version'] = BASELINE_VERSION
    encoded = json_bytes(config)
    name = hashlib.sha256(encoded).hexdigest() + '.json'
    manifest = [
        {
            'Config': name,
            'RepoTags': ['ghcr.io/kubijo/garmin-toolkit-hass:' + BASELINE_VERSION],
            'Layers': list(original.layers),
        }
    ]
    with destination.open('xb') as target, tarfile.open(fileobj=target, mode='w|gz', compresslevel=1) as output:
        copied: set[str] = set()
        with open_tar(source) as archive:
            for member in archive:
                if member.name in original.layers:
                    if not member.isfile() or member.name in copied:
                        raise ValueError('Fixture layers must be regular files')
                    with archive.extractfile(member) as payload:
                        output.addfile(member, payload)
                    copied.add(member.name)
        if copied != set(original.layers):
            raise ValueError('Fixture source is missing an image layer')
        for filename, contents in ((name, encoded), ('manifest.json', json_bytes(manifest))):
            member = tarfile.TarInfo(filename)
            member.size = len(contents)
            output.addfile(member, io.BytesIO(contents))


def validate_archive(archive: Path, guest: Platform, version: str, *, inspected: ImageArchive | None = None) -> None:
    config = (inspect_archive(archive) if inspected is None else inspected).configuration
    expected = 'arm64' if guest.architecture == 'aarch64' else 'amd64'
    if config.get('architecture') != expected or config.get('os') != 'linux':
        raise ValueError(f'App archive must target linux/{expected}; found {config.get("architecture")}')
    labels = object_value(object_value(config.get('config')).get('Labels'))
    expected_labels = {'io.hass.arch': guest.architecture, 'io.hass.version': version, 'io.hass.type': 'app'}
    if any(labels.get(key) != value for key, value in expected_labels.items()):
        raise ValueError(f'App archive Home Assistant labels differ from {expected_labels}')
