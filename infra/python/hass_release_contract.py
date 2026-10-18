"""Filesystem-backed contracts and content checks for local HASS release candidates."""

import hashlib
import re
import subprocess
import tomllib
from pathlib import Path
from typing import Literal, NotRequired, TypedDict, cast

from jinja2 import Environment, FileSystemLoader, StrictUndefined
from jsonschema import Draft202012Validator, validate
from jsonschema.exceptions import ValidationError

from json_data import file_sha256 as file_sha256
from json_data import json_bytes as json_bytes
from json_data import object_value
from json_data import read_json as read_json
from json_data import regular_file as regular_file

CONTRACT = Path(__file__).resolve().parents[1] / 'release'
type Architecture = Literal['amd64', 'aarch64']


class Source(TypedDict):
    repository: str
    tag: str
    commit: str
    lock_sha256: str


class Image(TypedDict):
    repository: str
    index_digest: str


class Platform(TypedDict):
    architecture: Architecture
    manifest_digest: str
    config_digest: str
    archive_sha256: str
    sbom_sha256: str
    builder_system: str
    derivation: str
    workflow_run: str | None


class Assembly(TypedDict):
    schema_version: int
    version: str
    source: Source
    image: Image
    platforms: list[Platform]
    source_parts: NotRequired[dict[str, int]]


class FileRecord(TypedDict):
    name: str
    role: str
    size: int
    sha256: str
    architecture: NotRequired[Architecture]


class Release(Assembly):
    status: Literal['candidate']
    acceptance: list[object]
    files: list[FileRecord]


class FileSpec(TypedDict):
    name: str
    role: str
    architecture: NotRequired[Architecture]
    source: NotRequired[str]


def validate_document(value: object, definition: Literal['assembly', 'release', 'native_build', 'registry']) -> None:
    schema = object_value(read_json(CONTRACT / 'hass-release.schema.json'))
    schema['$ref'] = f'#/$defs/{definition}'
    try:
        validate(value, schema, cls=Draft202012Validator)
    except ValidationError as error:
        location = '.'.join(str(part) for part in error.absolute_path) or '<root>'
        raise ValueError(f'Invalid {definition} at {location}: {error.message}') from error


def validate_identity(document: Assembly) -> None:
    if document['source']['tag'] != 'v' + document['version']:
        raise ValueError('Source tag and release version differ')
    architectures = [platform['architecture'] for platform in document['platforms']]
    if len(architectures) != len(set(architectures)):
        raise ValueError('Duplicate release platform')
    if 'source_parts' in document and set(document['source_parts']) != set(architectures):
        raise ValueError('Source part counts differ from the release platforms')
    for platform in document['platforms']:
        expected = 'x86_64-linux' if platform['architecture'] == 'amd64' else 'aarch64-linux'
        if platform['builder_system'] != expected:
            raise ValueError(f'Builder system differs from platform {platform["architecture"]}')


def environment() -> Environment:
    return Environment(
        loader=FileSystemLoader(CONTRACT), undefined=StrictUndefined, autoescape=False, keep_trailing_newline=True
    )


def inventory(document: Assembly) -> list[FileSpec]:
    configured = tomllib.loads((CONTRACT / 'hass-files.toml').read_text())
    if configured['version'] != 1:
        raise ValueError('Unsupported release inventory contract')
    templates = environment()
    files: list[FileSpec] = []
    for entry in configured['files']:
        files.append(
            cast(FileSpec, {**entry, 'name': templates.from_string(entry['name']).render(version=document['version'])})
        )
    for platform in document['platforms']:
        for entry in configured['platform_files']:
            count = document.get('source_parts', {}).get(platform['architecture'], 1)
            parts = range(2, count + 1) if entry['role'] == 'dependency_sources_part' else (1,)
            for part in parts:
                files.append(
                    cast(
                        FileSpec,
                        {
                            **entry,
                            'architecture': platform['architecture'],
                            'name': templates.from_string(entry['name']).render(
                                version=document['version'], architecture=platform['architecture'], part=f'{part:03}'
                            ),
                        },
                    )
                )
    names = [entry['name'] for entry in files]
    if len(names) != len(set(names)) or any(not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*', name) for name in names):
        raise ValueError('Release inventory has duplicate or unsafe filenames')
    return sorted(files, key=lambda entry: entry['name'])


def describe_file(directory: Path, spec: FileSpec) -> FileRecord:
    path = directory / spec['name']
    result: FileRecord = {
        'name': spec['name'],
        'role': spec['role'],
        'size': path.lstat().st_size,
        'sha256': file_sha256(path),
    }
    if 'architecture' in spec:
        result['architecture'] = spec['architecture']
    return result


def checksums(document: Release, manifest: bytes) -> bytes:
    files = [
        *document['files'],
        {'name': 'release.json', 'sha256': hashlib.sha256(manifest).hexdigest()},
    ]
    return (
        environment()
        .get_template('SHA256SUMS.j2')
        .render(files=sorted(files, key=lambda entry: entry['name']))
        .encode()
    )


def check_directory(directory: Path, expected: set[str]) -> None:
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError(f'Expected a release directory, not a symlink: {directory}')
    actual = {path.name for path in directory.iterdir()}
    if actual != expected:
        raise ValueError(
            f'Release inventory differs; missing={sorted(expected - actual)}, extra={sorted(actual - expected)}'
        )
    for name in sorted(actual):
        regular_file(directory / name)


def check_shape(directory: Path, prescription: str = 'hass-release.dirspec.toml') -> None:
    subprocess.run(
        ['dirspec', str(directory), '--spec', str(CONTRACT / prescription)],
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    )
