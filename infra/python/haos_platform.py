"""Architecture and image checks shared by disposable HAOS app runners."""

import hashlib
import json
import os
import platform
import tarfile
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, cast

from json_data import is_object, unique_object

Architecture = Literal['auto', 'amd64', 'aarch64']


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


def machine_arguments(guest: Platform, runtime: Path) -> list[str]:
    accelerator = acceleration(guest)
    if guest.architecture == 'amd64':
        return [
            'qemu-system-x86_64',
            '-machine',
            'q35',
            '-accel',
            accelerator,
            '-cpu',
            'host' if accelerator == 'kvm' else 'max',
            '-drive',
            f'if=pflash,format=raw,unit=0,readonly=on,file={guest.firmware}',
            '-drive',
            f'if=pflash,format=raw,unit=1,file={runtime / "ovmf-vars.fd"}',
        ]
    return [
        'qemu-system-aarch64',
        '-machine',
        'virt',
        '-accel',
        accelerator,
        '-cpu',
        'host' if accelerator == 'kvm' else 'cortex-a57',
        '-bios',
        str(guest.firmware),
        '-device',
        'ramfb',
    ]


@dataclass(frozen=True)
class ImageArchive:
    configuration: dict[str, object]
    config_digest: str
    repository_tags: tuple[str, ...]


def inspect_archive(archive: Path) -> ImageArchive:
    """Read bounded regular metadata members without extracting the image to disk."""
    with tarfile.open(archive, 'r:*') as image:

        def contents(name: str) -> bytes:
            matching = [member for member in image.getmembers() if member.name == name]
            if len(matching) != 1:
                raise ValueError(f'App archive metadata must occur exactly once: {name}')
            member = matching[0]
            if not member.isfile() or member.size > 1024 * 1024:
                raise ValueError(f'App archive metadata must be a bounded regular file: {name}')
            stream = image.extractfile(member)
            if stream is None:
                raise ValueError(f'App archive metadata is missing: {name}')
            with stream:
                return stream.read(1024 * 1024 + 1)

        manifest: object = json.loads(contents('manifest.json'), object_pairs_hook=unique_object)
        if not isinstance(manifest, list) or len(cast('list[object]', manifest)) != 1:
            raise ValueError('App archive must contain exactly one image')
        entry = cast('list[dict[str, object]]', manifest)[0]
        if not is_object(entry):
            raise ValueError('Invalid app archive manifest entry')
        config_name = entry.get('Config')
        if not isinstance(config_name, str):
            raise ValueError('App archive manifest has no image configuration')
        if Path(config_name).is_absolute() or '..' in Path(config_name).parts:
            raise ValueError('App archive image configuration path is unsafe')
        config_bytes = contents(config_name)
        config: object = json.loads(config_bytes, object_pairs_hook=unique_object)
        if not isinstance(config, dict):
            raise ValueError('Invalid app archive image configuration')
        tags = entry.get('RepoTags', [])
        if not isinstance(tags, list) or not all(isinstance(tag, str) for tag in cast('list[object]', tags)):
            raise ValueError('Invalid app archive repository tags')
        return ImageArchive(
            cast('dict[str, object]', config),
            'sha256:' + hashlib.sha256(config_bytes).hexdigest(),
            tuple(cast('list[str]', tags)),
        )


def archive_config(archive: Path) -> dict[str, object]:
    return inspect_archive(archive).configuration


def validate_archive(archive: Path, guest: Platform, version: str, *, inspected: ImageArchive | None = None) -> None:
    config = (inspect_archive(archive) if inspected is None else inspected).configuration
    expected = 'arm64' if guest.architecture == 'aarch64' else 'amd64'
    if config.get('architecture') != expected or config.get('os') != 'linux':
        raise ValueError(f'App archive must target linux/{expected}; found {config.get("architecture")}')
    settings = config.get('config')
    if not isinstance(settings, dict):
        raise ValueError('App archive has no runtime configuration')
    labels = cast('dict[str, object]', settings).get('Labels')
    if not isinstance(labels, dict):
        raise ValueError('App archive has no Home Assistant labels')
    labels = cast('dict[str, object]', labels)
    expected_labels = {'io.hass.arch': guest.architecture, 'io.hass.version': version, 'io.hass.type': 'app'}
    if any(labels.get(key) != value for key, value in expected_labels.items()):
        raise ValueError(f'App archive Home Assistant labels differ from {expected_labels}')
