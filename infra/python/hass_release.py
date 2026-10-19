"""Assemble HASS candidates and verify local artifacts or digest-addressed registry content."""

import errno
import hashlib
import json
import re
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
from hass_attestations import VerifyProvenance, attest, attest_payloads
from hass_image_sbom import MAX_SBOM_BYTES
from hass_publish import (
    CleanupStaging,
    CompletePublication,
    PreparePublication,
    PromoteDraft,
    ReleaseEvidence,
    ReleaseGuard,
    RollbackStore,
    StagePublication,
    cleanup_staging,
    complete_publication,
    package_evidence,
    prepare_publication,
    promote_draft,
    release_guard,
    rollback_store,
    stage_publication,
)
from hass_registry import PrepareRegistry, VerifyRegistry, prepare_registry, verify_registry
from hass_release_build import NativeBuild, record_build
from hass_release_contract import (
    Assembly,
    FileRecord,
    Payload,
    Release,
    check_directory,
    check_shape,
    checksums,
    describe_file,
    file_sha256,
    inventory,
    json_bytes,
    read_json,
    regular_file,
    validate_document,
    validate_identity,
)
from hass_release_source import admit_source, archive_source, source_bytes
from hass_repository import Repository, generate_repository
from hass_sources import BundleSources, bundle_sources, verify_sources
from json_data import MAX_JSON_BYTES, is_array, is_object, object_value, unique_object


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
            | Annotated[PrepareRegistry, tyro.conf.subcommand('prepare-registry', prefix_name=False)]
            | Annotated[VerifyRegistry, tyro.conf.subcommand('verify-registry', prefix_name=False)]
            | Annotated[VerifyProvenance, tyro.conf.subcommand('verify-provenance', prefix_name=False)]
            | Annotated[BundleSources, tyro.conf.subcommand('bundle-sources', prefix_name=False)]
            | Annotated[ReleaseGuard, tyro.conf.subcommand('guard-release', prefix_name=False)]
            | Annotated[PreparePublication, tyro.conf.subcommand('prepare-publication', prefix_name=False)]
            | Annotated[StagePublication, tyro.conf.subcommand('stage-publication', prefix_name=False)]
            | Annotated[ReleaseEvidence, tyro.conf.subcommand('release-evidence', prefix_name=False)]
            | Annotated[CompletePublication, tyro.conf.subcommand('complete-publication', prefix_name=False)]
            | Annotated[RollbackStore, tyro.conf.subcommand('rollback-store', prefix_name=False)]
            | Annotated[PromoteDraft, tyro.conf.subcommand('promote-draft', prefix_name=False)]
            | Annotated[CleanupStaging, tyro.conf.subcommand('cleanup-staging', prefix_name=False)]
        ),
        tyro.conf.arg(name=''),
    ]


def check_payload_metadata(directory: Path, document: Payload, *, require_resolved: bool = False) -> None:
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
        if inspected.manifest_digest != platform['manifest_digest']:
            raise ValueError(f'Image manifest digest differs from the receipt: {architecture}')
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
        verify_sources(
            by_role['dependency_sources'],
            by_role['source_inventory'],
            platform['archive_sha256'],
            require_resolved=require_resolved,
            part_count=document.get('source_parts', {}).get(architecture, 1),
        )


def copy_payload(source: Path, commit: str, document: Payload, artifacts: Path, output: Path) -> list[FileRecord]:
    specs = inventory(document)
    check_directory(artifacts, {entry['name'] for entry in specs if 'architecture' in entry})
    check_payload_metadata(artifacts, document)
    for entry in specs:
        destination = output / entry['name']
        if entry['role'] == 'source':
            archive_source(source, commit, document['version'], destination.resolve())
        elif 'source' in entry:
            destination.write_bytes(source_bytes(source, commit, entry['source']))
        else:
            shutil.copyfile(artifacts / entry['name'], destination, follow_symlinks=False)
    return [describe_file(output, entry) for entry in specs]


def assemble(options: Assemble) -> str:
    document = cast(Assembly, read_json(options.receipt))
    validate_document(document, 'assembly')
    validate_identity(document)
    admit_source(options.source, document)
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Output already exists; verify it explicitly instead of replacing it')
    if options.output.resolve().is_relative_to(options.artifacts.resolve()):
        raise ValueError('Output must be outside the build artifact input directory')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-release.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'candidate'
        staged.mkdir()
        files = copy_payload(options.source, document['source']['commit'], document, options.artifacts, staged)
        release: Release = {
            **document,
            'platforms': sorted(document['platforms'], key=lambda platform: platform['architecture']),
            'status': 'candidate',
            'acceptance': [],
            'files': files,
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


def verify(options: Verify) -> Release:
    if options.directory.is_symlink() or not options.directory.is_dir():
        raise ValueError('Expected a release directory, not a symlink')
    manifest_path = options.directory / 'release.json'
    regular_file(manifest_path)
    with manifest_path.open('rb') as source:
        raw = source.read(MAX_JSON_BYTES + 1)
    if len(raw) > MAX_JSON_BYTES:
        raise ValueError('Release manifest exceeds the size limit')
    if hashlib.sha256(raw).hexdigest() != options.manifest_sha256:
        raise ValueError('release.json differs from the expected SHA-256')
    document = cast(Release, json.loads(raw, object_pairs_hook=unique_object))
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
        if (options.directory / name).stat().st_size != entry['size']:
            raise ValueError(f'Release file size differs from the manifest: {name}')
    for name, entry in recorded.items():
        if entry != describe_file(options.directory, expected[name]):
            raise ValueError(f'Release file content or role differs from the manifest: {name}')
    manifest = json_bytes(document)
    if raw != manifest:
        raise ValueError('Release manifest is not canonically serialized')
    if (options.directory / 'SHA256SUMS').read_bytes() != checksums(document, manifest):
        raise ValueError('SHA256SUMS differs from the complete manifest inventory')
    check_payload_metadata(options.directory, document)
    check_shape(options.directory)
    return document


def verify_provenance(options: VerifyProvenance) -> None:
    if any(path.is_symlink() or not path.is_dir() for path in (options.directory, options.bundles)):
        raise ValueError('Expected release and attestation directories, not symlinks')
    path = options.directory / 'SHA256SUMS'
    regular_file(path)
    with path.open('rb') as source:
        authenticated = source.read(1024 * 1024 + 1)
    if len(authenticated) > 1024 * 1024:
        raise ValueError('Release checksums exceed the size limit')
    with tempfile.TemporaryDirectory(prefix='hass-provenance.') as scratch:
        snapshot = Path(scratch) / 'SHA256SUMS'
        snapshot.write_bytes(authenticated)
        attest(options, str(snapshot), 'checksums.sigstore.jsonl')
    matches = re.findall(rb'^([0-9a-f]{64})  release\.json$', authenticated, re.MULTILINE)
    if len(matches) != 1:
        raise ValueError('Authenticated checksums must name release.json exactly once')
    document = verify(
        Verify(
            directory=options.directory,
            manifest_sha256=matches[0].decode('ascii'),
            version=options.version,
            commit=options.commit,
        )
    )
    if authenticated != checksums(document, json_bytes(document)):
        raise ValueError('Authenticated checksums differ from the manifest inventory')
    attest(options, str(options.directory / 'release.json'), 'artifacts.sigstore.jsonl')
    attest_payloads(options, document)


def main(
    options: Assemble
    | Verify
    | NativeBuild
    | Repository
    | PrepareRegistry
    | VerifyRegistry
    | VerifyProvenance
    | BundleSources
    | ReleaseGuard
    | PreparePublication
    | StagePublication
    | ReleaseEvidence
    | CompletePublication
    | RollbackStore
    | CleanupStaging
    | PromoteDraft,
) -> int:
    output = TerminalOutput()
    output.install_tracebacks()
    try:
        if isinstance(options, ReleaseGuard):
            release_guard(options)
            return 0
        if isinstance(options, PreparePublication):
            prepare_publication(options)
            return 0
        if isinstance(options, CompletePublication):
            complete_publication(options)
            return 0
        if isinstance(options, ReleaseEvidence):
            package_evidence(options)
            return 0
        if isinstance(options, StagePublication):
            stage_publication(options)
            return 0
        if isinstance(options, CleanupStaging):
            cleanup_staging(options)
            return 0
        if isinstance(options, RollbackStore):
            rollback_store(options)
            return 0
        if isinstance(options, PromoteDraft):
            promote_draft(options)
            return 0
        if isinstance(options, Assemble):
            digest = assemble(options)
            output.panel(Text(str(options.output) + '\nrelease.json SHA-256: ' + digest), title='Candidate assembled')
        elif isinstance(options, Verify):
            verify(options)
        elif isinstance(options, VerifyProvenance):
            verify_provenance(options)
            output.panel(
                Text('Release artifacts and image digests match the trusted release workflow.'),
                title='Provenance verified',
                style='green',
            )
            return 0
        elif isinstance(options, NativeBuild):
            record_build(options)
            output.panel(Text(str(options.output)), title='Native build recorded', style='green')
            return 0
        elif isinstance(options, BundleSources):
            bundle_sources(options)
            output.panel(Text(str(options.output)), title='Dependency sources bundled', style='green')
            return 0
        elif isinstance(options, Repository):
            generate_repository(options)
            output.panel(Text(str(options.output)), title='App repository generated', style='green')
            return 0
        else:
            document = verify_registry(options) if isinstance(options, VerifyRegistry) else prepare_registry(options)
            output.panel(
                Text(str(options.output) + '\nOCI index: ' + str(object_value(document['image'])['index_digest'])),
                title='Registry content verified' if isinstance(options, VerifyRegistry) else 'OCI candidate prepared',
                style='green',
            )
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
