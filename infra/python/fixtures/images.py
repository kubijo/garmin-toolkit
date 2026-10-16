"""Deterministic project-owned Docker archives for admission and SBOM tests."""

import gzip
import hashlib
import io
import json
import tarfile
from collections.abc import Mapping
from pathlib import Path


def tar_bytes(files: Mapping[str, bytes | None]) -> bytes:
    result = io.BytesIO()
    with tarfile.open(fileobj=result, mode='w') as archive:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.size, member.mode = len(data) if data is not None else 0, 0o755
            member.type = tarfile.DIRTYPE if data is None else tarfile.REGTYPE
            archive.addfile(member, io.BytesIO(data) if data is not None else None)
    return result.getvalue()


def write_image(
    path: Path,
    *,
    files: Mapping[str, bytes | None] | None = None,
    architecture: str = 'amd64',
    hass_arch: str = 'amd64',
    version: str = '0.1.0',
    tag: str | None = None,
) -> str:
    layer = tar_bytes(files or {})
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
    archive = tar_bytes(
        {
            digest + '.json': config,
            'layer.tar': layer,
            'manifest.json': json.dumps(
                [
                    {
                        'Config': digest + '.json',
                        'Layers': ['layer.tar'],
                        'RepoTags': [tag or 'ghcr.io/kubijo/garmin-toolkit-hass:' + version],
                    }
                ]
            ).encode(),
        }
    )
    path.write_bytes(gzip.compress(archive, mtime=0) if path.suffix == '.gz' else archive)
    return 'sha256:' + digest
