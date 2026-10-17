"""Architecture and image checks shared by disposable HAOS app runners."""

import hashlib
import io
import json
import os
import platform
import re
import tarfile
from contextlib import nullcontext
from copy import deepcopy
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from archive_io import MAX_METADATA_BYTES, chunks, open_tar
from json_data import Object, json_bytes, object_rows, object_value, text, unique_object

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
    manifest: Object = field(default_factory=dict)
    manifest_digest: str = ''
    compressed_layers: frozenset[str] = frozenset()


def _read_archive(archive: Path, destination: Path | None = None) -> ImageArchive:
    metadata: dict[str, bytes] = {}
    blobs: dict[str, tuple[int, str]] = {}
    total = 0
    with open_tar(archive) as image:
        for member in image:
            if member.isdir() and member.name.rstrip('/') in ('blobs', 'blobs/sha256'):
                continue
            if member.name in blobs:
                raise ValueError(f'App archive metadata must occur exactly once: {member.name}')
            if not member.isfile() or not (
                member.name in ('index.json', 'oci-layout') or re.fullmatch(r'blobs/sha256/[0-9a-f]{64}', member.name)
            ):
                raise ValueError(f'Expected a regular native OCI archive member: {member.name}')
            with (
                image.extractfile(member) as stream,
                (destination / member.name).open('xb') if destination is not None else nullcontext() as output,
            ):
                first = stream.read(1)
                capture = member.name in ('index.json', 'oci-layout') or first in (b'{', b' ', b'\t', b'\r', b'\n')
                if capture and (member.size > MAX_METADATA_BYTES or total + member.size > 16 * MAX_METADATA_BYTES):
                    raise ValueError('App archive metadata exceeds the size limit')
                digest = hashlib.sha256(first)
                if output is not None:
                    output.write(first)
                data = bytearray(first) if capture else bytearray()
                for chunk in chunks(stream):
                    digest.update(chunk)
                    if output is not None:
                        output.write(chunk)
                    if capture:
                        data.extend(chunk)
            hashed = digest.hexdigest()
            if member.name.startswith('blobs/') and member.name.rsplit('/', 1)[1] != hashed:
                raise ValueError('OCI blob name differs from its content digest')
            blobs[member.name] = (member.size, 'sha256:' + hashed)
            if capture:
                metadata[member.name] = bytes(data)
                total += member.size

    def contents(name: str) -> bytes:
        if name not in metadata:
            raise ValueError(f'App archive metadata must occur exactly once: {name}')
        return metadata[name]

    def descriptor(value: Object, media_type: str) -> str:
        digest = text(value.get('digest'))
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', digest) or value.get('mediaType') != media_type:
            raise ValueError('Unexpected native OCI descriptor digest or media type')
        name = 'blobs/sha256/' + digest.removeprefix('sha256:')
        if type(value.get('size')) is not int or blobs.get(name) != (value['size'], digest):
            raise ValueError('Native OCI blob differs from its descriptor size/digest')
        if 'urls' in value or 'data' in value:
            raise ValueError('External or inline OCI blobs are unsupported')
        return name

    if json.loads(contents('oci-layout'), object_pairs_hook=unique_object) != {'imageLayoutVersion': '1.0.0'}:
        raise ValueError('Unexpected OCI layout version')
    index = object_value(json.loads(contents('index.json'), object_pairs_hook=unique_object))
    entries = object_rows(index.get('manifests'))
    if index.get('schemaVersion') != 2 or len(entries) != 1:
        raise ValueError('Native app archive must contain exactly one platform image')
    image = descriptor(entries[0], 'application/vnd.oci.image.manifest.v1+json')
    manifest = object_value(json.loads(contents(image), object_pairs_hook=unique_object))
    if (
        manifest.get('schemaVersion') != 2
        or manifest.get('mediaType') != 'application/vnd.oci.image.manifest.v1+json'
        or 'artifactType' in manifest
        or 'subject' in manifest
    ):
        raise ValueError('Expected an OCI container image manifest')
    config_name = descriptor(object_value(manifest.get('config')), 'application/vnd.oci.image.config.v1+json')
    config_bytes = contents(config_name)
    configuration = object_value(json.loads(config_bytes, object_pairs_hook=unique_object))
    layers: list[str] = []
    compressed: set[str] = set()
    for layer in object_rows(manifest.get('layers')):
        media_type = text(layer.get('mediaType'))
        if media_type not in ('application/vnd.oci.image.layer.v1.tar', 'application/vnd.oci.image.layer.v1.tar+gzip'):
            raise ValueError('Unsupported native OCI layer type')
        name = descriptor(layer, media_type)
        layers.append(name)
        if media_type.endswith('+gzip'):
            compressed.add(name)
    reference = text(object_value(entries[0].get('annotations')).get('org.opencontainers.image.ref.name'))
    return ImageArchive(
        configuration,
        blobs[config_name][1],
        (reference,),
        tuple(layers),
        manifest,
        blobs[image][1],
        frozenset(compressed),
    )


def inspect_archive(archive: Path) -> ImageArchive:
    """Verify a single-platform OCI archive without extracting or executing it."""
    return _read_archive(archive)


def unpack_archive(archive: Path, destination: Path) -> None:
    """Verify and copy OCI envelope files in one bounded pass, retaining no archive attributes."""
    destination.mkdir()
    (destination / 'blobs/sha256').mkdir(parents=True)
    _read_archive(archive, destination)


def baseline_fixture(source: Path, destination: Path) -> None:
    """Keep candidate layers unchanged; change only metadata for a private Supervisor upgrade fixture."""
    original = inspect_archive(source)
    config = deepcopy(original.configuration)
    labels = object_value(object_value(config['config'])['Labels'])
    if labels['io.hass.version'] == BASELINE_VERSION:
        raise ValueError('The candidate must differ from the fixture baseline version')
    labels['io.hass.version'] = BASELINE_VERSION
    encoded = json_bytes(config)
    digest = 'sha256:' + hashlib.sha256(encoded).hexdigest()
    name = 'blobs/sha256/' + digest.removeprefix('sha256:')
    manifest = deepcopy(original.manifest)
    manifest['config'] = {
        'mediaType': 'application/vnd.oci.image.config.v1+json',
        'digest': digest,
        'size': len(encoded),
    }
    if 'annotations' in manifest:
        object_value(manifest['annotations'])['org.opencontainers.image.version'] = BASELINE_VERSION
    manifest_bytes = json_bytes(manifest)
    manifest_digest = hashlib.sha256(manifest_bytes).hexdigest()
    index = {
        'schemaVersion': 2,
        'mediaType': 'application/vnd.oci.image.index.v1+json',
        'manifests': [
            {
                'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                'digest': 'sha256:' + manifest_digest,
                'size': len(manifest_bytes),
                'annotations': {
                    'org.opencontainers.image.ref.name': 'ghcr.io/kubijo/garmin-toolkit-hass:' + BASELINE_VERSION
                },
            }
        ],
    }
    with destination.open('xb') as target, tarfile.open(fileobj=target, mode='w|') as output:
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
        for filename, contents in (
            (name, encoded),
            ('blobs/sha256/' + manifest_digest, manifest_bytes),
            ('oci-layout', json_bytes({'imageLayoutVersion': '1.0.0'})),
            ('index.json', json_bytes(index)),
        ):
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
