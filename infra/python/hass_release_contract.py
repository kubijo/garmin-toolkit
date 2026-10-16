"""Filesystem-backed contracts and content checks for local HASS release candidates."""

import hashlib
import json
import re
import stat
import subprocess
import tomllib
from pathlib import Path
from typing import Any, Literal, NotRequired, TypedDict, cast

from jinja2 import Environment, FileSystemLoader, StrictUndefined
from jsonschema import Draft202012Validator, validate
from jsonschema.exceptions import ValidationError

from json_data import unique_object

CONTRACT = Path(__file__).resolve().parents[1] / 'release'
MAX_JSON_BYTES = 8 * 1024 * 1024
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


def regular_file(path: Path) -> None:
    """Reject links and special files before opening or hashing a release payload."""
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
        raise ValueError(f'Expected a nonempty regular file: {path}')


def file_sha256(path: Path) -> str:
    regular_file(path)
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def read_json(path: Path) -> Any:
    regular_file(path)
    with path.open('rb') as stream:
        data = stream.read(MAX_JSON_BYTES + 1)
    if len(data) > MAX_JSON_BYTES:
        raise ValueError(f'JSON metadata exceeds {MAX_JSON_BYTES} bytes: {path}')
    return json.loads(data, object_pairs_hook=unique_object)


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False) + '\n').encode()


def validate_document(value: object, definition: Literal['assembly', 'release']) -> None:
    schema = read_json(CONTRACT / 'hass-release.schema.json')
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
            files.append(
                cast(
                    FileSpec,
                    {
                        **entry,
                        'architecture': platform['architecture'],
                        'name': templates.from_string(entry['name']).render(
                            version=document['version'], architecture=platform['architecture']
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


def check_shape(directory: Path) -> None:
    subprocess.run(
        ['dirspec', str(directory), '--spec', str(CONTRACT / 'hass-release.dirspec.toml')],
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    )
