"""Prepare annotated OCI images and independently verify digest-addressed registry pulls."""

import gzip
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from archive_io import MAX_IMAGE_BYTES, MAX_METADATA_BYTES, LimitedReader, chunks, open_tar
from haos_platform import inspect_archive
from hass_release_build import verify_build_inputs
from hass_release_contract import check_shape, validate_document
from hass_release_source import git, source_bytes
from json_data import (
    Object,
    file_sha256,
    json_bytes,
    object_rows,
    object_value,
    read_json,
    strings,
    text,
    unique_object,
)

OCI = 'application/vnd.oci.image.'
INDEX, MANIFEST, CONFIG = OCI + 'index.v1+json', OCI + 'manifest.v1+json', OCI + 'config.v1+json'
LAYER = OCI + 'layer.v1.tar'
DIGEST = r'sha256:[0-9a-f]{64}'


@dataclass(frozen=True, kw_only=True)
class PrepareRegistry:
    """Create an annotated OCI layout from verified native exports without contacting a registry."""

    source: Path
    builds: list[Path]
    output: Path


@dataclass(frozen=True, kw_only=True)
class VerifyRegistry(PrepareRegistry):
    """Verify an immutable registry reference against native exports; retain only the receipt."""

    reference: str
    acceptance: tuple[Path, ...] = ()
    authfile: Path | None = None
    plain_http: bool = False


def native_inputs(source: Path, builds: list[Path]) -> Object:
    if not 1 <= len(builds) <= 2:
        raise ValueError('Expected one or two native build directories')
    documents: list[Object] = []
    for directory in builds:
        check_shape(directory, 'hass-native-build.dirspec.toml')
        archive = next(directory.glob('*.tar.gz'))
        document = verify_build_inputs(archive, directory / 'native-build.json', source)
        identity = object_value(document['source'])
        tag = identity.get('tag')
        if tag is not None and (
            tag != 'v' + text(document['version'])
            or git(source, 'rev-parse', '--verify', 'refs/tags/' + text(tag) + '^{commit}') != identity['commit']
        ):
            raise ValueError('Native receipt tag differs from its source commit/version')
        inspected = inspect_archive(archive)
        expected = strings(object_value(inspected.configuration.get('rootfs')).get('diff_ids'))
        actual: dict[str, str] = {}
        with open_tar(archive) as image:
            for member in image:
                if member.name in inspected.layers:
                    if not member.isfile() or member.name in actual:
                        raise ValueError('Expected distinct regular native image layers')
                    with image.extractfile(member) as stream:
                        digest = hashlib.sha256()
                        for chunk in chunks(stream):
                            digest.update(chunk)
                        actual[member.name] = 'sha256:' + digest.hexdigest()
        if (
            not expected
            or set(actual) != set(inspected.layers)
            or [actual[name] for name in inspected.layers] != expected
        ):
            raise ValueError('Native archive layers differ from configuration diff_ids')
        if documents and any(document[key] != documents[0][key] for key in ('source', 'version', 'image_repository')):
            raise ValueError('Native build source identities or versions differ')
        documents.append(document)
    platforms = [object_value(document['platform']) for document in documents]
    if len({text(platform['architecture']) for platform in platforms}) != len(platforms):
        raise ValueError('Duplicate native build platform')
    return {
        'schema_version': 1,
        'source': documents[0]['source'],
        'version': documents[0]['version'],
        'image': {'repository': documents[0]['image_repository']},
        'platforms': sorted(platforms, key=lambda platform: text(platform['architecture'])),
        'registry_reference': None,
        'acceptance': [],
    }


def annotations(source: Path, document: Object) -> Object:
    identity = object_value(document['source'])
    workspace = tomllib.loads(source_bytes(source, text(identity['commit']), 'Cargo.toml').decode())
    return {
        'org.opencontainers.image.source': identity['repository'],
        'org.opencontainers.image.revision': identity['commit'],
        'org.opencontainers.image.version': document['version'],
        'org.opencontainers.image.licenses': workspace['workspace']['package']['license'],
    }


def descriptor_digest(descriptor: Object, limit: int = MAX_IMAGE_BYTES) -> str:
    digest, size = text(descriptor.get('digest')), descriptor.get('size')
    if (
        not re.fullmatch(DIGEST, digest)
        or type(size) is not int
        or not 0 < size <= limit
        or 'urls' in descriptor
        or 'data' in descriptor
    ):
        raise ValueError('Invalid or external OCI descriptor')
    return digest


def blob_path(layout: Path, descriptor: Object) -> Path:
    digest = descriptor_digest(descriptor)
    path = layout / 'blobs' / 'sha256' / digest.removeprefix('sha256:')
    if path.lstat().st_size != descriptor['size'] or 'sha256:' + file_sha256(path) != digest:
        raise ValueError('OCI blob bytes differ from descriptor size/digest')
    return path


def metadata(layout: Path, descriptor: Object, media_type: str) -> Object:
    if descriptor.get('mediaType') != media_type:
        raise ValueError('Unexpected OCI descriptor media type')
    return object_value(read_json(blob_path(layout, descriptor)))


def write_blob(layout: Path, document: Object) -> Object:
    encoded = json_bytes(document)
    digest = hashlib.sha256(encoded).hexdigest()
    (layout / 'blobs' / 'sha256' / digest).write_bytes(encoded)
    return {'mediaType': document['mediaType'], 'digest': 'sha256:' + digest, 'size': len(encoded)}


def root_descriptor(layout: Path) -> Object:
    if read_json(layout / 'oci-layout') != {'imageLayoutVersion': '1.0.0'}:
        raise ValueError('Unexpected OCI layout version')
    wrapper = object_value(read_json(layout / 'index.json'))
    descriptors = object_rows(wrapper.get('manifests'))
    if wrapper.get('schemaVersion') != 2 or len(descriptors) != 1:
        raise ValueError('Expected exactly one OCI layout root')
    return descriptors[0]


def copy_image(source: str, destination: Path, *arguments: str) -> None:
    subprocess.run(
        [
            'skopeo',
            '--insecure-policy',
            '--tmpdir',
            str(destination.parent.resolve()),
            '--command-timeout',
            '10m',
            'copy',
            '--all',
            '--image-parallel-copies',
            '1',
            *arguments,
            source,
            'oci:' + str(destination),
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=630,
    )


def remote_manifest(options: VerifyRegistry, reference: str, scratch: Path) -> bytes:
    command = ['skopeo', '--command-timeout', '30s', 'inspect', '--raw']
    command += ['--authfile', str(options.authfile)] if options.authfile else ['--no-creds']
    command += ['--tls-verify=' + str(not options.plain_http).lower(), 'docker://' + reference]
    with tempfile.TemporaryFile(dir=scratch) as errors:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=errors)
        try:
            assert process.stdout is not None
            with process.stdout:
                raw = process.stdout.read(MAX_METADATA_BYTES + 1)
            if len(raw) > MAX_METADATA_BYTES:
                raise ValueError('Registry manifest exceeds the metadata size limit')
            code = process.wait(timeout=40)
            if code != 0:
                errors.seek(0)
                raise subprocess.CalledProcessError(
                    code, command, stderr=errors.read(MAX_METADATA_BYTES).decode(errors='replace')
                )
            return raw
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=5)


def preflight(options: VerifyRegistry, document: Object, scratch: Path) -> None:
    repository, _, expected = options.reference.partition('@')
    raw = remote_manifest(options, options.reference, scratch)
    if 'sha256:' + hashlib.sha256(raw).hexdigest() != expected:
        raise ValueError('Registry index bytes differ from the expected digest')
    index = object_value(json.loads(raw, object_pairs_hook=unique_object))
    manifests = object_rows(index.get('manifests'))
    if (
        index.get('schemaVersion') != 2
        or index.get('mediaType') != INDEX
        or len(manifests) != len(object_rows(document['platforms']))
    ):
        raise ValueError('Registry must expose the complete OCI image index')
    total = 0
    for descriptor in manifests:
        digest = descriptor_digest(descriptor, MAX_METADATA_BYTES)
        if descriptor.get('mediaType') != MANIFEST:
            raise ValueError('Registry index must reference image manifests directly')
        raw = remote_manifest(options, repository + '@' + digest, scratch)
        if len(raw) != descriptor['size'] or 'sha256:' + hashlib.sha256(raw).hexdigest() != digest:
            raise ValueError('Registry manifest bytes differ from the index descriptor')
        manifest = object_value(json.loads(raw, object_pairs_hook=unique_object))
        config = object_value(manifest.get('config'))
        descriptor_digest(config, MAX_METADATA_BYTES)
        if config.get('mediaType') != CONFIG:
            raise ValueError('Unexpected OCI configuration media type')
        layers = object_rows(manifest.get('layers'))
        if not layers:
            raise ValueError('Registry image must contain filesystem layers')
        for layer in layers:
            descriptor_digest(layer)
            total += cast(int, layer['size'])
            if layer.get('mediaType') not in (LAYER, LAYER + '+gzip') or total > MAX_IMAGE_BYTES:
                raise ValueError('Unsupported or oversized registry layer inventory')


def verify_layout(layout: Path, document: Object, expected_annotations: Object) -> None:
    root = root_descriptor(layout)
    if root['digest'] != object_value(document['image']).get('index_digest'):
        raise ValueError('OCI index differs from the expected registry digest')
    index = metadata(layout, root, INDEX)
    manifests = object_rows(index.get('manifests'))
    platforms = object_rows(document['platforms'])
    if (
        index.get('schemaVersion') != 2
        or index.get('mediaType') != INDEX
        or index.get('annotations') != expected_annotations
        or len(manifests) != len(platforms)
    ):
        raise ValueError('OCI index annotations or platform inventory differ')
    remaining = {text(platform['architecture']): platform for platform in platforms}
    total = 0
    for descriptor in manifests:
        platform = object_value(descriptor.get('platform'))
        architecture = text(platform.get('architecture'))
        architecture = 'aarch64' if architecture == 'arm64' else architecture
        expected = remaining.pop(architecture, None)
        if expected is None or platform != {
            'architecture': 'arm64' if architecture == 'aarch64' else architecture,
            'os': 'linux',
        }:
            raise ValueError('OCI index has a duplicate, missing, or substituted platform')
        manifest = metadata(layout, descriptor, MANIFEST)
        if (
            manifest.get('schemaVersion') != 2
            or manifest.get('mediaType') != MANIFEST
            or manifest.get('annotations') != expected_annotations
            or 'artifactType' in manifest
            or 'subject' in manifest
        ):
            raise ValueError('OCI manifest annotations or image type differ')
        config_descriptor = object_value(manifest.get('config'))
        if config_descriptor.get('digest') != expected['config_digest']:
            raise ValueError('Registry image configuration differs from the native export')
        config = metadata(layout, config_descriptor, CONFIG)
        if config.get('architecture') != platform['architecture'] or config.get('os') != 'linux':
            raise ValueError('OCI configuration platform differs from its index descriptor')
        layers = object_rows(manifest.get('layers'))
        diff_ids = strings(object_value(config.get('rootfs')).get('diff_ids'))
        if not layers or len(layers) != len(diff_ids):
            raise ValueError('OCI layer inventory differs from configuration diff_ids')
        for layer, expected_diff in zip(layers, diff_ids, strict=True):
            path = blob_path(layout, layer)
            media_type = layer.get('mediaType')
            if media_type not in (LAYER, LAYER + '+gzip'):
                raise ValueError('Unsupported OCI layer compression or media type')
            with gzip.open(path, 'rb') if media_type == LAYER + '+gzip' else path.open('rb') as raw:
                stream = LimitedReader(raw, MAX_IMAGE_BYTES - total)
                digest = hashlib.sha256()
                for chunk in chunks(stream):
                    digest.update(chunk)
                actual = 'sha256:' + digest.hexdigest()
                total += MAX_IMAGE_BYTES - total - stream.remaining
            if actual != expected_diff:
                raise ValueError('Registry layer differs from the native image filesystem')
        expected['manifest_digest'] = descriptor['digest']
    if remaining:
        raise ValueError('OCI index is missing native build platforms')


def check_output(options: PrepareRegistry) -> None:
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Registry output already exists')
    if any(options.output.resolve().is_relative_to(directory.resolve()) for directory in options.builds):
        raise ValueError('Registry output must be outside native build inputs')


def prepare_registry(options: PrepareRegistry) -> Object:
    check_output(options)
    document = native_inputs(options.source, options.builds)
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-registry.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'candidate'
        staged.mkdir()
        layout = staged / 'oci'
        (layout / 'blobs/sha256').mkdir(parents=True)
        (layout / 'oci-layout').write_bytes(json_bytes({'imageLayoutVersion': '1.0.0'}))
        expected = annotations(options.source, document)
        manifests: list[Object] = []
        for directory in options.builds:
            imported = Path(temporary) / 'import'
            archive = next(directory.glob('*.tar.gz'))
            original = inspect_archive(archive)
            copy_image('docker-archive:' + str(archive.resolve()), imported, '--format', 'oci')
            manifest = metadata(imported, root_descriptor(imported), MANIFEST)
            for blob in (imported / 'blobs/sha256').iterdir():
                blob.replace(layout / 'blobs/sha256' / blob.name)
            shutil.rmtree(imported)
            # Skopeo's OCI conversion rewrites config JSON; preserve the CI image's exact configuration bytes.
            (layout / 'blobs/sha256' / original.config_digest.removeprefix('sha256:')).write_bytes(
                original.configuration_bytes
            )
            manifest['config'] = {
                'mediaType': CONFIG,
                'digest': original.config_digest,
                'size': len(original.configuration_bytes),
            }
            manifest['annotations'] = expected
            descriptor = write_blob(layout, manifest)
            config = metadata(layout, object_value(manifest['config']), CONFIG)
            descriptor['platform'] = {'architecture': config['architecture'], 'os': config['os']}
            manifests.append(descriptor)
        manifests.sort(key=lambda descriptor: text(object_value(descriptor['platform'])['architecture']))
        index = write_blob(
            layout, {'schemaVersion': 2, 'mediaType': INDEX, 'manifests': manifests, 'annotations': expected}
        )
        (layout / 'index.json').write_bytes(json_bytes({'schemaVersion': 2, 'manifests': [index]}))
        object_value(document['image'])['index_digest'] = index['digest']
        verify_layout(layout, document, expected)
        validate_document(document, 'registry')
        (staged / 'registry.json').write_bytes(json_bytes(document))
        check_shape(staged, 'hass-registry.dirspec.toml')
        staged.rename(options.output)
    return document


def bind_acceptance(directories: tuple[Path, ...], document: Object) -> list[Object]:
    results: list[Object] = []
    platforms = {text(platform['architecture']): platform for platform in object_rows(document['platforms'])}
    for directory in directories:
        native = object_value(read_json(directory / 'native-build.json'))
        installed = object_value(read_json(directory / 'installation.json'))
        workflow = object_value(read_json(directory / 'workflow.json'))
        architecture = text(object_value(native['platform'])['architecture'])
        expected = platforms.pop(architecture, None)
        if expected is None or native['source'] != document['source'] or native['version'] != document['version']:
            raise ValueError('Acceptance source identity or platform differs from native builds')
        if object_value(native['platform']) != {
            key: value for key, value in expected.items() if key != 'manifest_digest'
        }:
            raise ValueError('Acceptance native receipt differs from the verified image')
        if (
            installed.get('config_digest') != expected['config_digest']
            or installed.get('archive_sha256') != expected['archive_sha256']
            or installed.get('architecture') != ('arm64' if architecture == 'aarch64' else architecture)
            or installed.get('version') != document['version']
            or workflow.get('passed') is not True
        ):
            raise ValueError('Acceptance did not pass for the verified native image')
        results.append(
            {
                'architecture': architecture,
                'scope': 'native-image-content',
                'runtime_registry_digest': text(installed.get('registry_digest')),
                'workflow_sha256': file_sha256(directory / 'workflow.json'),
                'installation_sha256': file_sha256(directory / 'installation.json'),
                'manifest_digest': expected['manifest_digest'],
            }
        )
    return results


def verify_registry(options: VerifyRegistry) -> Object:
    repository, separator, digest = options.reference.partition('@')
    local = re.fullmatch(r'127\.0\.0\.1:[1-9][0-9]*/garmin-toolkit-hass', repository) is not None
    if (
        not separator
        or not re.fullmatch(DIGEST, digest)
        or (repository != 'ghcr.io/kubijo/garmin-toolkit-hass' and not local)
        or (options.plain_http and not local)
    ):
        raise ValueError('Expected a digest-pinned canonical GHCR or loopback test registry reference')
    check_output(options)
    document = native_inputs(options.source, options.builds)
    object_value(document['image'])['index_digest'] = digest
    document['registry_reference'] = options.reference
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-registry.', dir=options.output.parent) as temporary:
        root = Path(temporary)
        preflight(options, document, root)
        arguments = ['--preserve-digests', '--src-tls-verify=' + str(not options.plain_http).lower()]
        arguments += ['--src-authfile', str(options.authfile)] if options.authfile else ['--src-no-creds']
        copy_image('docker://' + options.reference, root / 'oci', *arguments)
        verify_layout(root / 'oci', document, annotations(options.source, document))
        document['acceptance'] = bind_acceptance(options.acceptance, document)
        validate_document(document, 'registry')
        receipt = root / 'receipt'
        receipt.mkdir()
        (receipt / 'registry.json').write_bytes(json_bytes(document))
        receipt.rename(options.output)
    return document
