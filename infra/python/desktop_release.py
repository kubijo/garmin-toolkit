"""Build and attach AppImages to an existing GitHub release without replacing published bytes."""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path

from elftools.common.exceptions import ELFError
from elftools.elf.elffile import ELFFile

from github_releases import GitHubReleases, Releases
from json_data import object_rows, object_value, read_json, text

REPOSITORY = 'kubijo/garmin-toolkit'
ARCHITECTURES = {'x86_64': 'EM_X86_64', 'aarch64': 'EM_AARCH64'}
MODES = ('garmin-toolkit', 'garmin-toolkit-demo')
CHECKSUMS = 'DESKTOP-SHA256SUMS'
SYSTEMS = {'x86_64-linux': 'x86_64', 'aarch64-linux': 'aarch64'}
NATIVE_MANIFEST = 'desktop-build.json'
STAGED_MANIFEST = '.desktop-builds.json'


@dataclass(frozen=True)
class ReleaseSource:
    tag: str
    commit: str


def command(*args: str) -> str:
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE).stdout.rstrip('\n')


def source_identity(source: ReleaseSource) -> None:
    if not re.fullmatch(r'[0-9a-f]{40}', source.commit):
        raise ValueError('Release source must be an immutable Git commit')
    with Path('Cargo.toml').open('rb') as cargo_file:
        version = tomllib.load(cargo_file)['workspace']['package']['version']
    if not isinstance(version, str) or source.tag != f'v{version}':
        raise ValueError('Release tag must match the Cargo workspace version')
    if command('git', 'rev-parse', 'HEAD') != source.commit:
        raise ValueError('Checkout differs from the pinned release commit')
    if command('git', 'rev-parse', f'{source.tag}^{{commit}}') != source.commit:
        raise ValueError('Checkout differs from the release tag')
    if command('git', 'status', '--porcelain', '--untracked-files=normal'):
        raise ValueError('Release builds require a clean checkout')


def resolve_source(tag: str, expected_commit: str | None = None, *, releases: Releases | None = None) -> ReleaseSource:
    source = ReleaseSource(tag, command('git', 'rev-parse', 'HEAD'))
    if expected_commit and source.commit != expected_commit:
        raise ValueError('Release tag differs from the triggering commit')
    source_identity(source)
    assert_remote_source(source, releases or GitHubReleases(REPOSITORY))
    return source


def assert_remote_source(source: ReleaseSource, releases: Releases) -> None:
    if releases.tag_commit(source.tag) != source.commit:
        raise ValueError('Remote release tag differs from the pinned commit')


def asset_names() -> set[str]:
    return {
        f'{mode}-{architecture}.AppImage{suffix}'
        for mode in MODES
        for architecture in ARCHITECTURES
        for suffix in ('', '.zsync')
    }


def native_asset_names(system: str) -> set[str]:
    if system not in SYSTEMS:
        raise ValueError('Unsupported desktop release system')
    return {f'{mode}-{SYSTEMS[system]}.AppImage{suffix}' for mode in MODES for suffix in ('', '.zsync')}


def build_manifest(directory: Path, source: ReleaseSource, system: str) -> dict[str, object]:
    return {
        'schema_version': 1,
        'tag': source.tag,
        'commit': source.commit,
        'system': system,
        'assets': {name: digest(directory / name) for name in sorted(native_asset_names(system))},
    }


def verify_build_manifest(document: object, directory: Path, source: ReleaseSource) -> str:
    manifest = object_value(document)
    if set(manifest) != {'schema_version', 'tag', 'commit', 'system', 'assets'} or manifest['schema_version'] != 1:
        raise ValueError('Invalid desktop build manifest')
    if manifest['tag'] != source.tag or manifest['commit'] != source.commit:
        raise ValueError('Desktop artifacts differ from the pinned source')
    system = text(manifest['system'])
    assets = object_value(manifest['assets'])
    if set(assets) != native_asset_names(system):
        raise ValueError('Desktop native manifest has an invalid asset inventory')
    for name, expected in assets.items():
        if expected != digest(directory / name):
            raise ValueError(f'Desktop artifact differs from its native build: {name}')
    return system


def verify_staged_manifests(directory: Path, source: ReleaseSource) -> None:
    manifests = object_rows(read_json(directory / STAGED_MANIFEST))
    systems = [verify_build_manifest(manifest, directory, source) for manifest in manifests]
    if len(systems) != len(SYSTEMS) or set(systems) != set(SYSTEMS):
        raise ValueError('Desktop manifests must cover each native system exactly once')


def digest(path: Path, algorithm: str = 'sha256') -> str:
    with path.open('rb') as source:
        return hashlib.file_digest(source, algorithm).hexdigest()


def update_information(path: Path, architecture: str) -> str:
    """Inspect the runtime ELF section without executing another architecture's AppImage."""
    with path.open('rb') as source:
        try:
            elf = ELFFile(source)
            source.seek(8)
            if (
                elf.elfclass != 64
                or not elf.little_endian
                or source.read(3) != b'AI\x02'
                or elf.header['e_machine'] != ARCHITECTURES[architecture]
            ):
                raise ValueError(f'Invalid {architecture} type-2 AppImage: {path.name}')
            section = elf.get_section_by_name('.upd_info')
            if section is None:
                raise ValueError('AppImage has no embedded update information')
            if section.data_size != 1024:
                raise ValueError('Invalid AppImage update-information section size')
            data = section.data()
            if len(data) != 1024:
                raise ValueError('Truncated AppImage update information')
            return data.split(b'\0', 1)[0].decode('utf-8')
        except ELFError as error:
            raise ValueError(f'Invalid {architecture} type-2 AppImage: {path.name}') from error


def validate_assets(directory: Path, tag: str) -> list[Path]:
    expected = asset_names()
    actual = {path.name for path in directory.iterdir()} - {CHECKSUMS, STAGED_MANIFEST}
    if actual != expected:
        raise ValueError(f'Desktop asset inventory differs: missing={expected - actual}, extra={actual - expected}')
    for architecture in ARCHITECTURES:
        for mode in MODES:
            name = f'{mode}-{architecture}.AppImage'
            image = directory / name
            expected_info = f'gh-releases-zsync|kubijo|garmin-toolkit|latest-all|{name}.zsync'
            if update_information(image, architecture) != expected_info:
                raise ValueError(f'Wrong update channel or architecture: {name}')
            validate_control(image, directory / (name + '.zsync'), tag)
    return [directory / name for name in sorted(expected)]


def control_without_mtime(data: bytes) -> bytes:
    header, separator, blocks = data.partition(b'\n\n')
    if not separator:
        raise ValueError('zsync control has no header terminator')
    # Artifact transport changes mtime, which is not part of the file's content identity.
    headers = [line for line in header.splitlines() if not line.startswith(b'MTime: ')]
    return b'\n'.join(headers) + separator + blocks


def validate_control(image: Path, control: Path, tag: str) -> None:
    with tempfile.TemporaryDirectory(prefix='desktop-zsync-') as temporary:
        regenerated = Path(temporary) / 'expected.zsync'
        subprocess.run(
            [
                'zsyncmake',
                '-f',
                image.name,
                '-u',
                f'https://github.com/{REPOSITORY}/releases/download/{tag}/{image.name}',
                '-o',
                str(regenerated),
                str(image),
            ],
            check=True,
            capture_output=True,
        )
        expected = regenerated.read_bytes()
        if control.stat().st_size > len(expected) + 128:
            raise ValueError(f'zsync metadata exceeds the expected size: {image.name}')
        if control_without_mtime(control.read_bytes()) != control_without_mtime(expected):
            raise ValueError(f'zsync metadata does not describe the final AppImage: {image.name}')


def checksums(paths: list[Path]) -> str:
    return ''.join(f'{digest(path)}  {path.name}\n' for path in paths)


def build(source: ReleaseSource, system: str, output: Path) -> None:
    source_identity(source)
    names = native_asset_names(system)
    if output.exists() or output.is_symlink():
        raise ValueError('Desktop native output already exists')
    output.parent.mkdir(parents=True, exist_ok=True)
    link = output.with_name(output.name + '.nix')
    subprocess.run(
        [
            'nix',
            'build',
            f'.#packages.{system}.desktop-release-assets',
            '--out-link',
            str(link),
            '--print-build-logs',
            '--no-update-lock-file',
        ],
        check=True,
    )
    source_identity(source)
    if {path.name for path in link.iterdir()} != names:
        raise ValueError('Nix desktop asset inventory differs')
    output.mkdir(exist_ok=False)
    for name in sorted(names):
        shutil.copyfile(link / name, output / name)
    (output / NATIVE_MANIFEST).write_text(json.dumps(build_manifest(output, source, system), sort_keys=True) + '\n')


def stage(source: ReleaseSource, inputs: Path, output: Path) -> None:
    source_identity(source)
    output.mkdir(parents=True, exist_ok=False)
    manifests: list[dict[str, object]] = []
    systems: set[str] = set()
    for native in sorted(inputs.iterdir()):
        manifest = read_json(native / NATIVE_MANIFEST)
        system = verify_build_manifest(manifest, native, source)
        if system in systems:
            raise ValueError('Duplicate desktop native system')
        systems.add(system)
        names = native_asset_names(system)
        if {path.name for path in native.iterdir()} != names | {NATIVE_MANIFEST}:
            raise ValueError('Unexpected desktop build artifact')
        manifests.append(object_value(manifest))
        for name in sorted(names):
            path = native / name
            destination = output / path.name
            if destination.exists():
                raise ValueError(f'Duplicate desktop asset: {path.name}')
            shutil.copyfile(path, destination)
    if systems != set(SYSTEMS):
        raise ValueError('Desktop builds must cover both native systems')
    (output / STAGED_MANIFEST).write_text(json.dumps(manifests, sort_keys=True) + '\n')
    paths = validate_assets(output, source.tag)
    (output / CHECKSUMS).write_text(checksums(paths))


def publish(source: ReleaseSource, directory: Path, *, releases: Releases | None = None) -> None:
    source_identity(source)
    paths = validate_assets(directory, source.tag)
    verify_staged_manifests(directory, source)
    checksum_file = directory / CHECKSUMS
    if checksum_file.read_text() != checksums(paths):
        raise ValueError('Desktop checksums differ from staged assets')
    releases = releases or GitHubReleases(REPOSITORY)
    assert_remote_source(source, releases)
    if releases.is_draft(source.tag):
        raise ValueError('Desktop assets require the existing published release')
    existing = {asset.name: asset.digest for asset in releases.assets(source.tag)}
    missing: list[Path] = []
    for path in [*paths, checksum_file]:
        if path.name not in existing:
            missing.append(path)
            continue
        remote_digest = existing[path.name]
        if not remote_digest:
            with tempfile.TemporaryDirectory() as temporary:
                releases.download(source.tag, Path(temporary), pattern=path.name)
                remote_digest = 'sha256:' + digest(Path(temporary) / path.name)
        if remote_digest != 'sha256:' + digest(path):
            raise ValueError(f'Refusing to replace published desktop asset: {path.name}')
    if missing:
        assert_remote_source(source, releases)
        releases.upload(source.tag, missing)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='phase', required=True)
    resolve_parser = commands.add_parser('resolve')
    resolve_parser.add_argument('--tag', required=True)
    resolve_parser.add_argument('--expected-commit')
    resolve_parser.add_argument('--output', type=Path, required=True)
    build_parser = commands.add_parser('build')
    build_parser.add_argument('--system', required=True)
    build_parser.add_argument('--output', type=Path, required=True)
    stage_parser = commands.add_parser('stage')
    stage_parser.add_argument('--inputs', type=Path, required=True)
    stage_parser.add_argument('--output', type=Path, required=True)
    publish_parser = commands.add_parser('publish')
    publish_parser.add_argument('--directory', type=Path, required=True)
    for child in [build_parser, stage_parser, publish_parser]:
        child.add_argument('--tag', required=True)
        child.add_argument('--commit', required=True)
    args = parser.parse_args()
    if args.phase == 'resolve':
        source = resolve_source(args.tag, args.expected_commit)
        with args.output.open('a') as output:
            output.write(f'commit={source.commit}\n')
        return
    source = ReleaseSource(args.tag, args.commit)
    if args.phase == 'build':
        build(source, args.system, args.output)
    elif args.phase == 'stage':
        stage(source, args.inputs, args.output)
    else:
        publish(source, args.directory)


if __name__ == '__main__':
    main()
