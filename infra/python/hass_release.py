"""Assemble and verify local HASS release candidates; performs no publication or network access."""

import errno
import hashlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, cast

import tyro
from rich.text import Text

from cli_output import TerminalOutput
from haos_platform import PLATFORMS, inspect_archive, validate_archive
from hass_image_sbom import MAX_SBOM_BYTES
from hass_release_build import NativeBuild, record_build
from hass_release_contract import (
    Assembly,
    Release,
    check_directory,
    check_shape,
    checksums,
    describe_file,
    file_sha256,
    inventory,
    json_bytes,
    read_json,
    validate_document,
    validate_identity,
)
from hass_release_source import admit_source, archive_source, source_bytes
from hass_repository import Repository, generate_repository
from json_data import is_array, is_object


@dataclass(frozen=True, kw_only=True)
class Assemble:
    """Assemble an unsigned candidate from a detached clean source tag and native build receipts."""

    source: Path
    receipt: Path
    artifacts: Path
    output: Path


@dataclass(frozen=True, kw_only=True)
class Verify:
    """Verify local content against an independently trusted release.json hash and identity."""

    directory: Path
    manifest_sha256: str
    version: str
    commit: str


@dataclass(frozen=True, kw_only=True)
class Options:
    command: Annotated[
        (
            Annotated[Assemble, tyro.conf.subcommand('assemble', prefix_name=False)]
            | Annotated[Verify, tyro.conf.subcommand('verify', prefix_name=False)]
            | Annotated[NativeBuild, tyro.conf.subcommand('record-build', prefix_name=False)]
            | Annotated[Repository, tyro.conf.subcommand('repository', prefix_name=False)]
        ),
        tyro.conf.arg(name=''),
    ]


def check_payload_metadata(directory: Path, document: Assembly) -> None:
    specs = inventory(document)
    for platform in document['platforms']:
        architecture = platform['architecture']
        by_role = {
            entry['role']: directory / entry['name'] for entry in specs if entry.get('architecture') == architecture
        }
        receipt_hashes = {'image_archive': platform['archive_sha256'], 'sbom': platform['sbom_sha256']}
        for role, expected_hash in receipt_hashes.items():
            if file_sha256(by_role[role]) != expected_hash:
                raise ValueError(f'Build artifact hash differs from the receipt: {architecture} {role}')
        archive = by_role['image_archive']
        inspected = inspect_archive(archive)
        validate_archive(archive, PLATFORMS[architecture], document['version'], inspected=inspected)
        if inspected.config_digest != platform['config_digest']:
            raise ValueError(f'Image config digest differs from the receipt: {architecture}')
        expected_tag = document['image']['repository'] + ':' + document['version']
        if inspected.repository_tags != (expected_tag,):
            raise ValueError(f'Image repository/version differs from the receipt: {architecture}')
        sbom = read_json(by_role['sbom'], max_bytes=MAX_SBOM_BYTES)
        if (
            not is_object(sbom)
            or sbom.get('spdxVersion') != 'SPDX-2.3'
            or not is_array(sbom.get('packages'))
            or not sbom['packages']
        ):
            raise ValueError(f'Expected a nonempty SPDX 2.3 package inventory: {architecture}')


def assemble(options: Assemble) -> str:
    document = cast(Assembly, read_json(options.receipt))
    validate_document(document, 'assembly')
    validate_identity(document)
    admit_source(options.source, document)
    specs = inventory(document)
    inputs = {entry['name'] for entry in specs if 'architecture' in entry}
    check_directory(options.artifacts, inputs)
    check_payload_metadata(options.artifacts, document)
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Output already exists; verify it explicitly instead of replacing it')
    if options.output.resolve().is_relative_to(options.artifacts.resolve()):
        raise ValueError('Output must be outside the build artifact input directory')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-release.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'candidate'
        staged.mkdir()
        for entry in specs:
            destination = staged / entry['name']
            if entry['role'] == 'source':
                archive_source(options.source, document, destination.resolve())
            elif 'source' in entry:
                destination.write_bytes(source_bytes(options.source, document['source']['commit'], entry['source']))
            else:
                origin = options.artifacts / entry['name']
                # copyfile preserves bytes without inheriting read-only source permissions.
                shutil.copyfile(origin, destination, follow_symlinks=False)
        release: Release = {
            **document,
            'platforms': sorted(document['platforms'], key=lambda platform: platform['architecture']),
            'status': 'candidate',
            'acceptance': [],
            'files': [describe_file(staged, entry) for entry in specs],
        }
        validate_document(release, 'release')
        manifest = json_bytes(release)
        digest = hashlib.sha256(manifest).hexdigest()
        (staged / 'release.json').write_bytes(manifest)
        (staged / 'SHA256SUMS').write_bytes(checksums(release, manifest))
        verify(
            Verify(
                directory=staged,
                manifest_sha256=digest,
                version=document['version'],
                commit=document['source']['commit'],
            )
        )
        # A failed admission never exposes a partial candidate directory.
        staged.rename(options.output)
    return digest


def verify(options: Verify) -> None:
    if options.directory.is_symlink() or not options.directory.is_dir():
        raise ValueError('Expected a release directory, not a symlink')
    manifest_path = options.directory / 'release.json'
    if file_sha256(manifest_path) != options.manifest_sha256:
        raise ValueError('release.json differs from the expected SHA-256')
    document = cast(Release, read_json(manifest_path))
    validate_document(document, 'release')
    validate_identity(document)
    if document['version'] != options.version or document['source']['commit'] != options.commit:
        raise ValueError('Release differs from the expected source commit/version')
    specs = inventory(document)
    expected = {entry['name']: entry for entry in specs}
    recorded = {entry['name']: entry for entry in document['files']}
    if len(recorded) != len(document['files']) or recorded.keys() != expected.keys():
        raise ValueError('Manifest file inventory has duplicate, missing, or unexpected entries')
    check_directory(options.directory, {*expected, 'release.json', 'SHA256SUMS'})
    for name, entry in recorded.items():
        if entry != describe_file(options.directory, expected[name]):
            raise ValueError(f'Release file content or role differs from the manifest: {name}')
    manifest = json_bytes(document)
    if manifest_path.read_bytes() != manifest:
        raise ValueError('Release manifest is not canonically serialized')
    if (options.directory / 'SHA256SUMS').read_bytes() != checksums(document, manifest):
        raise ValueError('SHA256SUMS differs from the complete manifest inventory')
    check_payload_metadata(options.directory, document)
    check_shape(options.directory)


def main(options: Assemble | Verify | NativeBuild | Repository) -> int:
    output = TerminalOutput()
    output.install_tracebacks()
    try:
        if isinstance(options, Assemble):
            digest = assemble(options)
            output.panel(Text(str(options.output) + '\nrelease.json SHA-256: ' + digest), title='Candidate assembled')
        elif isinstance(options, Verify):
            verify(options)
        elif isinstance(options, NativeBuild):
            record_build(options)
            output.panel(Text(str(options.output)), title='Native build recorded', style='green')
            return 0
        else:
            generate_repository(options)
            output.panel(Text(str(options.output)), title='App repository generated', style='green')
            return 0
        output.panel(
            Text('Local candidate content verified. Registry provenance and HAOS qualification are not checked.'),
            title='Local verification',
            style='green',
        )
        return 0
    except subprocess.CalledProcessError as error:
        output.ansi((error.stdout or '') + (error.stderr or ''), title='Release check failed', error=True)
        return error.returncode if error.returncode > 0 else 128 - error.returncode
    except subprocess.TimeoutExpired:
        output.panel(Text('A release check timed out.'), title='Release check failed', error=True)
        return 124
    except (ValueError, OSError, KeyError, tarfile.TarError) as error:
        output.panel(Text(str(error)), title='Release check failed', error=True)
        if isinstance(error, OSError) and error.errno in (errno.ENOENT, errno.EACCES):
            return 127 if error.errno == errno.ENOENT else 126
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == '__main__':
    sys.exit(main(tyro.cli(Options).command))
