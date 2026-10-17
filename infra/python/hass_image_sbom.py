"""Produce and verify image-bound SPDX inventories with one set of enrichment rules."""

import hashlib
import os
import subprocess
import tempfile
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import cast
from urllib.parse import quote

from archive_io import expand
from haos_platform import ImageArchive, inspect_archive
from hass_image_inventory import ImageFiles, inventory_image
from json_data import Object, file_sha256, json_bytes, object_rows, object_value, read_json, text

MAX_SBOM_BYTES = 128 * 1024 * 1024
CONTRACT = Path(__file__).resolve().parents[1] / 'release'
IMAGE_ID = 'SPDXRef-GarminImage'
type Edge = tuple[str, str, str]
type FileRecords = dict[str, tuple[str, str | None]]


def image_version(inspected: ImageArchive) -> str:
    return text(object_value(object_value(inspected.configuration.get('config')).get('Labels')).get('io.hass.version'))


def identifier(kind: str, value: str) -> str:
    return 'SPDXRef-' + kind + '-' + hashlib.sha256(value.encode()).hexdigest()[:24]


def cargo_reference(name: str, version: str) -> Object:
    return {
        'referenceCategory': 'PACKAGE-MANAGER',
        'referenceType': 'purl',
        'referenceLocator': 'pkg:cargo/' + quote(name) + '@' + quote(version),
    }


def indexed(value: object, key: str) -> dict[str, Object]:
    result: dict[str, Object] = {}
    for row in object_rows(value):
        identity = text(row.get(key))
        if identity in result:
            raise ValueError(f'Duplicate SPDX identity: {identity}')
        result[identity] = row
    return result


def file_records(document: Object, files: ImageFiles) -> FileRecords:
    records: FileRecords = {}
    for entry in object_rows(document.get('files')):
        name = '/' + text(entry.get('fileName')).removeprefix('./').lstrip('/')
        if name in records or '..' in Path(name).parts:
            raise ValueError(f'Duplicate or unsafe SPDX file: {name}')
        hashes = [
            text(checksum.get('checksumValue'))
            for checksum in object_rows(entry.get('checksums', []))
            if checksum.get('algorithm') == 'SHA256'
        ]
        if len(hashes) > 1:
            raise ValueError(f'Ambiguous SPDX file hash: {name}')
        records[name] = (text(entry.get('SPDXID')), hashes[0] if hashes else None)
    if any(name not in records or records[name][1] != digest for name, digest in files.hashes.items()):
        raise ValueError('SPDX file coverage differs from the image')
    return records


@dataclass
class Inventory:
    packages: dict[str, Object] = field(default_factory=dict)
    relationships: set[Edge] = field(default_factory=set)
    licenses: dict[str, Object] = field(default_factory=dict)

    @classmethod
    def read(cls, document: Object, records: FileRecords) -> Inventory:
        packages = indexed(document.get('packages'), 'SPDXID')
        for package in packages.values():
            text(package.get('name'))
            if 'versionInfo' in package:
                text(package['versionInfo'])
        identities = {text(document.get('SPDXID')), *packages, *(record[0] for record in records.values())}
        if len(identities) != 1 + len(packages) + len(records):
            raise ValueError('Duplicate SPDX identity across document, files, or packages')
        edges = {
            (text(row.get('spdxElementId')), text(row.get('relationshipType')), text(row.get('relatedSpdxElement')))
            for row in object_rows(document.get('relationships'))
        }
        if any(left not in identities or right not in identities | {'NONE', 'NOASSERTION'} for left, _, right in edges):
            raise ValueError('SPDX relationship refers to an unknown identity')
        return cls(packages, edges, indexed(document.get('hasExtractedLicensingInfos', []), 'licenseId'))


def package(spdx_id: str, name: str, **fields: object) -> Object:
    return {
        'SPDXID': spdx_id,
        'name': name,
        'downloadLocation': 'NOASSERTION',
        'filesAnalyzed': False,
        'licenseConcluded': 'NOASSERTION',
        'licenseDeclared': 'NOASSERTION',
        'copyrightText': 'NOASSERTION',
        **fields,
    }


def supplemental(files: ImageFiles, records: FileRecords, bundle: object, digest: str, version: str) -> Inventory:
    result = Inventory(
        packages={
            IMAGE_ID: package(
                IMAGE_ID,
                'garmin-toolkit-hass',
                versionInfo=version,
                checksums=[{'algorithm': 'SHA256', 'checksumValue': digest}],
            )
        }
    )
    for root in sorted(files.store_paths):
        key = identifier('NixOutput', root)
        result.packages[key] = package(
            key,
            root.split('/')[-1],
            sourceInfo='Nix runtime output: ' + root,
            filesAnalyzed=True,
            licenseInfoFromFiles=['NOASSERTION'],
        )
        result.relationships.add((IMAGE_ID, 'CONTAINS', key))
        result.relationships.update(
            (key, 'CONTAINS', row[0]) for name, row in records.items() if name == root or name.startswith(root + '/')
        )
    for name, dependencies in sorted(files.wasm.items()):
        for dependency in dependencies:
            key = identifier('WasmCrate', name + ':' + dependency['name'] + ':' + dependency['version'])
            result.packages[key] = package(
                key,
                dependency['name'],
                versionInfo=dependency['version'],
                sourceInfo='cargo-auditable WebAssembly runtime dependency: ' + name,
                externalRefs=[cargo_reference(dependency['name'], dependency['version'])],
            )
            result.relationships.add((records[name][0], 'DEPENDS_ON', key))
    notices = object_value(bundle)
    raw_texts = notices.get('license_texts')
    if not isinstance(raw_texts, list):
        raise ValueError('Expected bundled license texts')
    texts = cast(list[object], raw_texts)
    assets = [
        row for row in object_rows(notices.get('entries')) if object_value(row.get('source')).get('kind') == 'asset'
    ]
    if not assets:
        raise ValueError('The bundled asset inventory is empty')
    for asset in assets:
        name, version = text(asset.get('name')), text(asset.get('version'))
        key = identifier('Asset', name + ':' + version)
        if key in result.packages:
            raise ValueError('Duplicate bundled asset')
        license_id = 'LicenseRef-' + key.removeprefix('SPDXRef-')
        terms: list[str] = []
        for notice in object_rows(asset.get('notices')):
            index = notice.get('text_index')
            if type(index) is not int or not 0 <= index < len(texts):
                raise ValueError('Invalid bundled license text index')
            terms.append(text(texts[index]))
        if not terms or not '\n\n'.join(terms).strip():
            raise ValueError('Bundled assets require actual license text')
        result.licenses[license_id] = {
            'licenseId': license_id,
            'extractedText': '\n\n'.join(terms),
            'name': text(asset.get('license')),
        }
        result.packages[key] = package(
            key,
            name,
            versionInfo=version,
            licenseDeclared=license_id,
            downloadLocation=text(object_value(asset.get('source')).get('url')),
            sourceInfo='Bundled asset from the reviewed HASS notice inventory',
        )
        result.relationships.add((IMAGE_ID, 'CONTAINS', key))
    return result


def verify_inventory(document: Object, files: ImageFiles, bundle: object, digest: str, version: str) -> None:
    if document.get('spdxVersion') != 'SPDX-2.3' or document.get('dataLicense') != 'CC0-1.0':
        raise ValueError('Expected an SPDX 2.3 document')
    records = file_records(document, files)
    actual = Inventory.read(document, records)
    expected = supplemental(files, records, bundle, digest, version)
    expected.relationships.add((text(document.get('SPDXID')), 'DESCRIBES', IMAGE_ID))
    managed = {
        key
        for key in actual.packages
        if key == IMAGE_ID or key.startswith(('SPDXRef-NixOutput-', 'SPDXRef-WasmCrate-', 'SPDXRef-Asset-'))
    }
    if document.get('documentDescribes') != [IMAGE_ID] or managed != expected.packages.keys():
        raise ValueError('SPDX image/component inventory differs from the image')
    if any(actual.packages.get(key) != value for key, value in expected.packages.items()):
        raise ValueError('SPDX package contents differ from the image or reviewed assets')
    if not expected.relationships.issubset(actual.relationships) or any(
        actual.licenses.get(key) != value for key, value in expected.licenses.items()
    ):
        raise ValueError('SPDX relationships or licenses differ from the image inventory')
    native: dict[tuple[str, str], set[str]] = {}
    for key, row in actual.packages.items():
        if key.startswith('SPDXRef-Package-rust-crate-'):
            identity = text(row.get('name')), text(row.get('versionInfo'))
            if cargo_reference(*identity) not in object_rows(row.get('externalRefs')):
                raise ValueError('SPDX native Rust package reference differs from its identity')
            native.setdefault(identity, set()).add(key)
    for name, dependencies in files.native.items():
        for row in dependencies:
            if not any(
                (key, 'OTHER', records[name][0]) in actual.relationships
                for key in native.get((row['name'], row['version']), ())
            ):
                raise ValueError('SPDX native Rust dependency/file association differs from the image')
    if any(
        row['version'] != version
        for dependencies in (*files.native.values(), *files.wasm.values())
        for row in dependencies
        if row['name'] in {'garmin-hass', 'garmin-gpx-worker', 'garmin-hass-web'}
    ):
        raise ValueError('Application crate versions differ from the image version')


def verify_sbom(archive: Path, sbom: Path, bundle: object, *, inspected: ImageArchive | None = None) -> None:
    inspected = inspected or inspect_archive(archive)
    verify_inventory(
        object_value(read_json(sbom, max_bytes=MAX_SBOM_BYTES)),
        inventory_image(archive, inspected),
        bundle,
        file_sha256(archive),
        image_version(inspected),
    )


def generate(archive: Path, closure: Path, bundle: Path, output: Path, created: int) -> None:
    if output.exists() or output.is_symlink():
        raise ValueError('SPDX output already exists')
    inspected = inspect_archive(archive)
    files = inventory_image(archive, inspected)
    if files.store_paths != set(closure.read_text().splitlines()):
        raise ValueError('Actual image store paths differ from the producing Nix runtime closure')
    scratch_root = Path(os.environ.get('NIX_BUILD_TOP', Path(__file__).resolve().parents[2] / '.tmp'))
    scratch_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hass-sbom.', dir=scratch_root) as temporary:
        scratch = Path(temporary)
        expanded = expand(archive, scratch / 'image.tar')
        document_path = scratch / 'scan.spdx.json'
        version, digest, notices = image_version(inspected), file_sha256(archive), read_json(bundle)
        subprocess.run(
            [
                'syft',
                'scan',
                'oci-archive:' + str(expanded),
                '--config',
                str(CONTRACT / 'syft.yaml'),
                '--source-name',
                'garmin-toolkit-hass',
                '--source-version',
                version,
                '-o',
                'spdx-json=' + str(document_path),
            ],
            # Nix gives build output a terminal, but Syft's interactive UI cannot read build input.
            env=os.environ | {'NO_TTY': '1'},
            check=True,
            timeout=900,
        )
        document = object_value(read_json(document_path, max_bytes=MAX_SBOM_BYTES))
        # Syft represents an explicit image-root directory with an empty file name.
        for entry in object_rows(document.get('files')):
            if entry.get('fileName') == '':
                entry['fileName'] = '/'
        records = file_records(document, files)
        actual, extra = Inventory.read(document, records), supplemental(files, records, notices, digest, version)
        if actual.packages.keys() & extra.packages.keys():
            raise ValueError('Scanner used reserved package identities')
        document.update(
            packages=[*actual.packages.values(), *extra.packages.values()],
            documentDescribes=[IMAGE_ID],
            name='garmin-toolkit-hass-' + digest,
            documentNamespace='https://github.com/kubijo/garmin-toolkit/sbom/' + digest,
            hasExtractedLicensingInfos=[*actual.licenses.values(), *extra.licenses.values()],
        )
        edges = actual.relationships | extra.relationships | {(text(document.get('SPDXID')), 'DESCRIBES', IMAGE_ID)}
        document['relationships'] = [
            dict(zip(('spdxElementId', 'relationshipType', 'relatedSpdxElement'), edge, strict=True))
            for edge in sorted(edges)
        ]
        object_value(document.get('creationInfo'))['created'] = datetime.fromtimestamp(created, UTC).strftime(
            '%Y-%m-%dT%H:%M:%SZ'
        )
        verify_inventory(document, files, notices, digest, version)
        payload = json_bytes(document)
        if len(payload) > MAX_SBOM_BYTES:
            raise ValueError('SPDX output exceeds its size limit')
        destination = output.open('xb')
        try:
            with destination:
                destination.write(payload)
        except BaseException:
            output.unlink(missing_ok=True)
            raise


if __name__ == '__main__':
    import tyro

    tyro.cli(generate)
