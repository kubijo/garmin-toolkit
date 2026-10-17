"""Deterministic project-owned OCI archives for admission and SBOM tests."""

import gzip
import hashlib
import io
import json
import tarfile
from collections.abc import Mapping
from pathlib import Path


def tar_bytes(files: Mapping[str, bytes | None], *, hardlinks: Mapping[str, str] | None = None) -> bytes:
    result = io.BytesIO()
    with tarfile.open(fileobj=result, mode='w') as archive:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.size, member.mode = len(data) if data is not None else 0, 0o755
            member.type = tarfile.DIRTYPE if data is None else tarfile.REGTYPE
            archive.addfile(member, io.BytesIO(data) if data is not None else None)
        for name, target in (hardlinks or {}).items():
            member = tarfile.TarInfo(name)
            member.type, member.linkname = tarfile.LNKTYPE, target
            archive.addfile(member)
    return result.getvalue()


def write_image(
    path: Path,
    *,
    files: Mapping[str, bytes | None] | None = None,
    architecture: str = 'amd64',
    hass_arch: str = 'amd64',
    version: str = '0.1.0',
    tag: str | None = None,
    annotations: Mapping[str, str] | None = None,
    hardlinks: Mapping[str, str] | None = None,
) -> str:
    layer = tar_bytes(files or {}, hardlinks=hardlinks)
    config = json.dumps(
        {
            'architecture': architecture,
            'os': 'linux',
            'config': {
                'Labels': {'io.hass.version': version, 'io.hass.type': 'app', 'io.hass.arch': hass_arch},
                'Volumes': {'/data': {}},
                'ExposedPorts': {'8099/tcp': {}},
                'Env': ['GARMIN_TOOLKIT_HASS_DEVICE_SOURCE=raw-mtp', 'GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY=172.30.32.2'],
            },
            'rootfs': {'type': 'layers', 'diff_ids': ['sha256:' + hashlib.sha256(layer).hexdigest()]},
        }
    ).encode()
    digest = hashlib.sha256(config).hexdigest()
    compressed = gzip.compress(layer, mtime=0)
    layer_digest = hashlib.sha256(compressed).hexdigest()
    manifest = json.dumps(
        {
            'schemaVersion': 2,
            'mediaType': 'application/vnd.oci.image.manifest.v1+json',
            'annotations': dict(annotations or {}),
            'config': {
                'mediaType': 'application/vnd.oci.image.config.v1+json',
                'digest': 'sha256:' + digest,
                'size': len(config),
            },
            'layers': [
                {
                    'mediaType': 'application/vnd.oci.image.layer.v1.tar+gzip',
                    'digest': 'sha256:' + layer_digest,
                    'size': len(compressed),
                }
            ],
        }
    ).encode()
    manifest_digest = hashlib.sha256(manifest).hexdigest()
    archive = tar_bytes(
        {
            'blobs/sha256/' + digest: config,
            'blobs/sha256/' + layer_digest: compressed,
            'blobs/sha256/' + manifest_digest: manifest,
            'oci-layout': b'{"imageLayoutVersion":"1.0.0"}',
            'index.json': json.dumps(
                {
                    'schemaVersion': 2,
                    'mediaType': 'application/vnd.oci.image.index.v1+json',
                    'manifests': [
                        {
                            'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                            'digest': 'sha256:' + manifest_digest,
                            'size': len(manifest),
                            'annotations': {
                                'org.opencontainers.image.ref.name': tag
                                or 'ghcr.io/kubijo/garmin-toolkit-hass:' + version
                            },
                        }
                    ],
                }
            ).encode(),
        }
    )
    path.write_bytes(gzip.compress(archive, mtime=0) if path.suffix == '.gz' else archive)
    return 'sha256:' + digest
