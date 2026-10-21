"""Authenticate a published image before using it as a disposable upgrade baseline."""

import hashlib
import json
import subprocess
from pathlib import Path

from haos_platform import Platform, inspect_archive, validate_archive
from hass_attestations import VerifyProvenance, attest
from hass_release_contract import validate_document
from hass_release_source import git, source_metadata
from json_data import Object, file_sha256, object_rows, object_value, read_json, regular_file, text

REPOSITORY = 'https://github.com/kubijo/garmin-toolkit'
IMAGE = 'ghcr.io/kubijo/garmin-toolkit-hass'


def published_baseline(directory: Path, source: Path, guest: Platform) -> tuple[Path, Object]:
    """Verify the signed release inventory and selected image without downloading unrelated source bundles."""
    manifest = directory / 'release.json'
    document = object_value(read_json(manifest))
    validate_document(document, 'release')
    version = text(document['version'])
    identity = object_value(document['source'])
    tag = 'v' + version
    commit = git(source, 'rev-parse', '--verify', 'refs/tags/' + tag + '^{commit}')
    if identity.get('repository') != REPOSITORY or identity.get('tag') != tag or identity.get('commit') != commit:
        raise ValueError('Published baseline differs from the independently selected release tag')
    actual_version, app = source_metadata(source, commit)
    if actual_version != version or app.get('version') != version or app.get('image') != IMAGE:
        raise ValueError('Published baseline metadata differs from its committed source')
    image = object_value(document['image'])
    if image.get('repository') != IMAGE:
        raise ValueError('Published baseline must use the canonical app image')
    options = VerifyProvenance(directory=directory, bundles=directory, version=version, commit=commit)
    published = object_value(
        json.loads(
            subprocess.run(
                ['gh', 'release', 'view', tag, '--repo', 'kubijo/garmin-toolkit', '--json', 'tagName,isDraft'],
                check=True,
                capture_output=True,
                text=True,
                timeout=60,
            ).stdout
        )
    )
    if published.get('tagName') != tag or published.get('isDraft') is not False:
        raise ValueError('Upgrade baseline must be an already published release')
    sums = directory / 'SHA256SUMS'
    regular_file(sums)
    if sums.stat().st_size > 1024 * 1024:
        raise ValueError('Published baseline checksums exceed the size limit')
    attest(options, str(sums), 'checksums.sigstore.jsonl')
    authenticated = sums.read_bytes().splitlines()

    def checksum(path: Path) -> str:
        regular_file(path)
        digest = file_sha256(path)
        if authenticated.count((digest + '  ' + path.name).encode()) != 1:
            raise ValueError('Published baseline file differs from authenticated checksums: ' + path.name)
        return digest

    checksum(manifest)
    attest(options, str(manifest), 'artifacts.sigstore.jsonl')
    platforms = [row for row in object_rows(document['platforms']) if row.get('architecture') == guest.architecture]
    files = [
        row
        for row in object_rows(document['files'])
        if row.get('role') == 'image_archive' and row.get('architecture') == guest.architecture
    ]
    if len(platforms) != 1 or len(files) != 1:
        raise ValueError('Published baseline must contain exactly one image for the selected architecture')
    platform, record = platforms[0], files[0]
    name = text(record['name'])
    if Path(name).name != name:
        raise ValueError('Published baseline image must be a direct release asset')
    archive = directory / name
    regular_file(archive)
    if archive.stat().st_size != record.get('size'):
        raise ValueError('Published baseline image size differs from its signed inventory')
    digest = checksum(archive)
    if digest != record.get('sha256') or digest != platform.get('archive_sha256'):
        raise ValueError('Published baseline image differs from its signed platform receipt')
    attest(options, str(archive), 'artifacts.sigstore.jsonl')
    inspected = inspect_archive(archive)
    validate_archive(archive, guest, version, inspected=inspected)
    if inspected.config_digest != platform.get('config_digest') or inspected.manifest_digest != platform.get(
        'manifest_digest'
    ):
        raise ValueError('Published baseline OCI identity differs from its signed platform receipt')
    return archive, {
        'release': tag,
        'version': version,
        'source_commit': commit,
        'archive_sha256': digest,
        'manifest_digest': inspected.manifest_digest,
        'config_digest': inspected.config_digest,
        'registry_index_digest': image['index_digest'],
        'release_manifest_sha256': hashlib.sha256(manifest.read_bytes()).hexdigest(),
        'signatures_verified': True,
    }
