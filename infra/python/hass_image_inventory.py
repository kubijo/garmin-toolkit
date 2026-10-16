"""Read the actual Nix-built image without extracting or executing its contents."""

import hashlib
import json
import re
import struct
import zlib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import TypedDict, cast

from archive_io import MAX_MEMBERS, chunks, open_tar
from haos_platform import ImageArchive
from json_data import object_rows, object_value, text, unique_object

STORE_PATH = re.compile(r'^/nix/store/[0-9a-df-np-sv-z]{32}-[^/]+')
MAX_AUDIT_BYTES = 8 * 1024 * 1024
MAX_WASM_BYTES = 128 * 1024 * 1024
MAX_NATIVE_BYTES = 512 * 1024 * 1024


class Dependency(TypedDict):
    name: str
    version: str


def audit_packages(data: bytes) -> list[Dependency]:
    decoder = zlib.decompressobj()
    decoded = decoder.decompress(data, MAX_AUDIT_BYTES + 1)
    if len(decoded) > MAX_AUDIT_BYTES or not decoder.eof or decoder.unused_data:
        raise ValueError('Invalid or oversized Rust dependency metadata')
    document = object_value(json.loads(decoded, object_pairs_hook=unique_object))
    packages: list[Dependency] = []
    for entry in object_rows(document.get('packages')):
        if entry.get('kind', 'runtime') not in ('runtime', 'build'):
            raise ValueError('Invalid cargo-auditable dependency kind')
        if entry.get('kind', 'runtime') == 'runtime':
            packages.append({'name': text(entry.get('name')), 'version': text(entry.get('version'))})
    return packages


def native_dependencies(data: bytes, *, expected_machine: int | None = None) -> list[Dependency]:
    """Only the two supported Linux image targets use this ELF64 little-endian format."""
    if len(data) < 64 or not data.startswith(b'\x7fELF\x02\x01'):
        raise ValueError('Expected a native ELF64 little-endian executable')
    if expected_machine is not None and struct.unpack_from('<H', data, 18)[0] != expected_machine:
        raise ValueError('Native executable architecture differs from the image')
    table = cast(int, struct.unpack_from('<Q', data, 40)[0])
    size, count, strings_index = cast(tuple[int, int, int], struct.unpack_from('<HHH', data, 58))
    if size != 64 or not count or strings_index >= count or table + size * count > len(data):
        raise ValueError('Invalid native ELF section table')
    sections = [
        cast(tuple[int, ...], struct.unpack_from('<IIQQQQIIQQ', data, table + size * index)) for index in range(count)
    ]
    strings = sections[strings_index]
    names = data[strings[4] : strings[4] + strings[5]]
    audits: list[bytes] = []
    for section in sections:
        if section[0] >= len(names) or (section[1] != 8 and section[4] + section[5] > len(data)):
            raise ValueError('Invalid native ELF section')
        name = names[section[0] :].split(b'\0', 1)[0]
        if name == b'.dep-v0':
            audits.append(data[section[4] : section[4] + section[5]])
    if len(audits) != 1:
        raise ValueError('Native executable must retain exactly one cargo-auditable dependency section')
    return audit_packages(audits[0])


def image_path(name: str) -> str:
    path = PurePosixPath(name)
    if '..' in path.parts:
        raise ValueError(f'Unsafe image member: {name}')
    # Nix layers include absolute container-root entries; no members are extracted on the host.
    return '/' + str(path).removeprefix('./').lstrip('/')


def unsigned_leb(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 35, 7):
        if offset >= len(data):
            break
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7F) << shift
        if byte < 128 and value < 2**32:
            return value, offset
    raise ValueError('Invalid WebAssembly section length')


def wasm_dependencies(data: bytes) -> list[Dependency]:
    if not data.startswith(b'\0asm\x01\0\0\0'):
        raise ValueError('Invalid WebAssembly module')
    offset = 8
    audits: list[bytes] = []
    while offset < len(data):
        section = data[offset]
        size, begin = unsigned_leb(data, offset + 1)
        end = begin + size
        if end > len(data):
            raise ValueError('Truncated WebAssembly section')
        if section == 0:
            length, start = unsigned_leb(data, begin)
            if start + length > end:
                raise ValueError('Truncated WebAssembly custom section')
            if data[start : start + length] == b'.dep-v0':
                audits.append(data[start + length : end])
        offset = end
    if len(audits) != 1:
        raise ValueError('WebAssembly must retain exactly one cargo-auditable dependency section')
    packages = audit_packages(audits[0])
    if not any(package['name'] == 'garmin-hass-web' for package in packages):
        raise ValueError('WebAssembly dependency graph is missing the app root')
    return packages


@dataclass(frozen=True)
class ImageFiles:
    hashes: dict[str, str]
    store_paths: set[str]
    wasm: dict[str, list[Dependency]]
    native: dict[str, list[Dependency]]


def inventory_image(archive: Path, inspected: ImageArchive) -> ImageFiles:
    architecture = inspected.configuration.get('architecture')
    if not isinstance(architecture, str) or architecture not in ('amd64', 'arm64'):
        raise ValueError('Unsupported native image architecture')
    expected_machine = 62 if architecture == 'amd64' else 183
    if not inspected.layers or len(set(inspected.layers)) != len(inspected.layers):
        raise ValueError('Expected distinct image layers')
    layers: set[str] = set()
    hashes: dict[str, str] = {}
    seen: dict[str, bool] = {}
    store_paths: set[str] = set()
    wasm: dict[str, list[Dependency]] = {}
    native: dict[str, list[Dependency]] = {}
    count = 0
    with open_tar(archive) as outer:
        for member in outer:
            if member.name not in inspected.layers:
                continue
            if member.name in layers or not member.isfile():
                raise ValueError('Duplicate or nonregular image layer')
            stream = outer.extractfile(member)
            layers.add(member.name)
            with open_tar(stream) as layer:
                for entry in layer:
                    count += 1
                    if count > MAX_MEMBERS:
                        raise ValueError('Image exceeds the total layer member limit')
                    name = image_path(entry.name)
                    if name in seen and not (seen[name] and entry.isdir()):
                        raise ValueError(f'Duplicate image file or conflicting entry: {name}')
                    seen[name] = entry.isdir()
                    if any(part.startswith('.wh.') for part in PurePosixPath(name).parts) or entry.islnk():
                        raise ValueError('Expected Nix image layers without whiteouts or hardlinks')
                    if root := STORE_PATH.match(name):
                        store_paths.add(root[0])
                    if not entry.isfile():
                        if not (entry.isdir() or entry.issym()):
                            raise ValueError(f'Unsupported image entry: {name}')
                        continue
                    contents = layer.extractfile(entry)
                    with contents:
                        is_wasm = '/share/garmin-hass/web/' in name and name.endswith('.wasm')
                        is_native = PurePosixPath(name).name in (
                            'garmin-hass',
                            '.garmin-hass-wrapped',
                            'garmin-gpx-worker',
                        )
                        if is_wasm or is_native:
                            if entry.size > (MAX_WASM_BYTES if is_wasm else MAX_NATIVE_BYTES):
                                raise ValueError('Oversized application binary')
                            data = b''.join(chunks(contents))
                            if is_wasm:
                                wasm[name] = wasm_dependencies(data)
                            elif data.startswith(b'\x7fELF'):
                                native[name] = native_dependencies(data, expected_machine=expected_machine)
                            digest = hashlib.sha256(data).hexdigest()
                        else:
                            hasher = hashlib.sha256()
                            for chunk in chunks(contents):
                                hasher.update(chunk)
                            digest = hasher.hexdigest()
                    hashes[name] = digest
    if layers != set(inspected.layers):
        raise ValueError('Image layer inventory is incomplete')
    roots = {entry['name'] for packages in native.values() for entry in packages}
    if not store_paths or not wasm or not {'garmin-hass', 'garmin-gpx-worker'}.issubset(roots):
        raise ValueError('Image is missing its Nix closure, frontend, or native Rust metadata')
    return ImageFiles(hashes, store_paths, wasm, native)
