"""Record actual native Nix build outputs without inventing registry identities."""

import hashlib
import json
import platform
import shutil
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path

from archive_io import check_archive
from haos_platform import PLATFORMS, inspect_archive, validate_archive
from hass_image_sbom import MAX_SBOM_BYTES, verify_sbom
from hass_release_contract import (
    CONTRACT,
    check_shape,
    environment,
    file_sha256,
    json_bytes,
    read_json,
    validate_document,
)
from hass_release_source import clean_commit, source_bytes, source_metadata
from json_data import Object, object_value, strings, text, unique_object


@dataclass(frozen=True, kw_only=True)
class NativeBuild:
    """Record a native image and its verified SBOM; no build or publication is performed."""

    source: Path
    archive: Path
    sbom: Path
    derivation: Path
    output: Path
    workflow_run: str | None = None


def build_source(source: Path) -> tuple[str, Object]:
    commit = clean_commit(source)
    version, app = source_metadata(source, commit)
    if app['version'] != version or app['image'] != 'ghcr.io/kubijo/garmin-toolkit-hass':
        raise ValueError('Workspace and Home Assistant image metadata differ')
    tag = 'v' + version
    tagged = subprocess.run(
        ['git', '-C', str(source), 'rev-parse', '--verify', 'refs/tags/' + tag + '^{commit}'],
        check=False,
        capture_output=True,
        text=True,
    )
    return version, {
        'repository': 'https://github.com/kubijo/garmin-toolkit',
        'commit': commit,
        'tag': tag if tagged.returncode == 0 and tagged.stdout.strip() == commit else None,
        'lock_sha256': hashlib.sha256(source_bytes(source, commit, 'flake.lock')).hexdigest(),
    }


def native_derivation(path: Path) -> tuple[str, Path]:
    result = subprocess.run(
        ['nix', 'derivation', 'show', str(path)], check=True, capture_output=True, text=True, timeout=60
    )
    derivations = object_value(json.loads(result.stdout, object_pairs_hook=unique_object))
    key = str(path)
    if 'derivations' in derivations:
        if derivations.get('version') != 4:
            raise ValueError('Unsupported Nix derivation JSON version')
        derivations = object_value(derivations['derivations'])
        key = path.name
    if set(derivations) != {key}:
        raise ValueError('Expected exactly the producing image derivation')
    producing = object_value(derivations[key])
    output = Path(text(object_value(object_value(producing.get('outputs')).get('out')).get('path')))
    if not output.is_absolute():
        if len(output.parts) != 1:
            raise ValueError('Invalid relative Nix output path')
        output = path.parent / output
    return text(producing.get('system')), output


def verify_build_inputs(archive: Path, receipt: Path, source: Path) -> Object:
    """Bind acceptance inputs to an unsigned native receipt and committed source material."""
    check_shape(receipt.parent, 'hass-native-build.dirspec.toml')
    document = object_value(read_json(receipt))
    validate_document(document, 'native_build')
    identity, producing = object_value(document['source']), object_value(document['platform'])
    commit = text(identity['commit'])
    version, app = source_metadata(source, commit)
    if version != document['version'] or app.get('image') != document['image_repository']:
        raise ValueError('Native receipt version or image differs from its committed source')
    if hashlib.sha256(source_bytes(source, commit, 'flake.lock')).hexdigest() != identity['lock_sha256']:
        raise ValueError('Native receipt lockfile differs from its committed source')
    exported = next(receipt.parent.glob('*.oci.tar'))
    sbom = next(receipt.parent.glob('*.spdx.json'))
    if not archive.samefile(exported):
        raise ValueError('Acceptance archive is not the native receipt export')
    check_archive(archive)
    if sbom.stat().st_size > MAX_SBOM_BYTES:
        raise ValueError('Native SPDX inventory exceeds the size limit')
    if file_sha256(archive) != producing['archive_sha256'] or file_sha256(sbom) != producing['sbom_sha256']:
        raise ValueError('Native build payload hashes differ from the receipt')
    inspected = inspect_archive(archive)
    validate_archive(archive, PLATFORMS[text(producing['architecture'])], version, inspected=inspected)
    if inspected.config_digest != producing['config_digest']:
        raise ValueError('Native receipt config digest differs from its archive')
    if inspected.manifest_digest != producing['manifest_digest']:
        raise ValueError('Native receipt manifest digest differs from its archive')
    bundle = json.loads(
        source_bytes(source, commit, 'assets/licenses/bundle-hass.json'), object_pairs_hook=unique_object
    )
    verify_sbom(archive, sbom, bundle, inspected=inspected)
    return document


def record_build(options: NativeBuild) -> None:
    version, source = build_source(options.source)
    inspected = inspect_archive(options.archive)
    architecture = 'aarch64' if inspected.configuration.get('architecture') == 'arm64' else 'amd64'
    validate_archive(options.archive, PLATFORMS[architecture], version, inspected=inspected)
    settings = object_value(inspected.configuration.get('config'))
    if (
        settings.get('Volumes') != {'/data': {}}
        or settings.get('ExposedPorts') != {'8099/tcp': {}}
        or not {
            'GARMIN_TOOLKIT_HASS_DEVICE_SOURCE=raw-mtp',
            'GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY=172.30.32.2',
        }.issubset(strings(settings.get('Env')))
    ):
        raise ValueError('Production image runtime configuration differs from the Home Assistant contract')
    if inspected.repository_tags != ('ghcr.io/kubijo/garmin-toolkit-hass:' + version,):
        raise ValueError('Image repository/version differs from the source')
    builder = 'aarch64-linux' if architecture == 'aarch64' else 'x86_64-linux'
    host = {'x86_64': 'x86_64-linux', 'aarch64': 'aarch64-linux'}.get(platform.machine())
    if platform.system() != 'Linux' or host != builder:
        raise ValueError('Build receipts require the matching native Linux host')
    system, image_output = native_derivation(options.derivation)
    if system != builder or image_output != options.archive.resolve():
        raise ValueError('Image output or native builder differs from the Nix derivation')
    bundle: object = json.loads(
        source_bytes(options.source, text(source['commit']), 'assets/licenses/bundle-hass.json'),
        object_pairs_hook=unique_object,
    )
    verify_sbom(options.archive, options.sbom, bundle, inspected=inspected)
    hashes = {'image_archive': file_sha256(options.archive), 'sbom': file_sha256(options.sbom)}
    receipt: Object = {
        'schema_version': 1,
        'version': version,
        'source': source,
        'image_repository': 'ghcr.io/kubijo/garmin-toolkit-hass',
        'platform': {
            'architecture': architecture,
            'config_digest': inspected.config_digest,
            'manifest_digest': inspected.manifest_digest,
            'archive_sha256': hashes['image_archive'],
            'sbom_sha256': hashes['sbom'],
            'builder_system': builder,
            'derivation': str(options.derivation),
            'workflow_run': options.workflow_run,
        },
    }
    validate_document(receipt, 'native_build')
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Native build output already exists')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-native-build.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'build'
        staged.mkdir()
        templates = environment()
        configured = tomllib.loads((CONTRACT / 'hass-files.toml').read_text())
        inputs = {'image_archive': options.archive, 'sbom': options.sbom}
        for entry in configured['platform_files']:
            name = templates.from_string(entry['name']).render(version=version, architecture=architecture)
            shutil.copyfile(inputs[entry['role']], staged / name, follow_symlinks=False)
            if file_sha256(staged / name) != hashes[entry['role']]:
                raise ValueError('Native build input changed while copying')
        (staged / 'native-build.json').write_bytes(json_bytes(receipt))
        subprocess.run(
            [
                'dirspec',
                str(staged),
                '--spec',
                str(CONTRACT / 'hass-native-build.dirspec.toml'),
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=60,
        )
        staged.rename(options.output)
