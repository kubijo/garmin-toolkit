"""Read-only source admission for a tagged HASS release."""

import hashlib
import subprocess
import tomllib
from pathlib import Path

import yaml

from hass_release_contract import Assembly
from json_data import Object, object_value, strings, text


def git(source: Path, *arguments: str) -> str:
    return subprocess.check_output(['git', '-C', str(source), *arguments], text=True, stderr=subprocess.PIPE).strip()


def source_bytes(source: Path, commit: str, name: str) -> bytes:
    """Use committed blobs even when a checkout has ignored or assume-unchanged files."""
    entry = git(source, 'ls-tree', commit, '--', name)
    if not entry.startswith(('100644 blob ', '100755 blob ')):
        raise ValueError(f'Expected a tracked regular source file: {name}')
    return subprocess.check_output(['git', '-C', str(source), 'show', commit + ':' + name])


def clean_commit(source: Path) -> str:
    if Path(git(source, 'rev-parse', '--show-toplevel')).resolve() != source.resolve():
        raise ValueError('Source must be the repository root')
    if git(source, 'status', '--porcelain=v1', '--untracked-files=all'):
        raise ValueError('Release source tree is dirty')
    return git(source, 'rev-parse', '--verify', 'HEAD')


def source_metadata(source: Path, commit: str) -> tuple[str, Object]:
    version = text(
        tomllib.loads(source_bytes(source, commit, 'Cargo.toml').decode())['workspace']['package']['version']
    )
    return version, object_value(yaml.safe_load(source_bytes(source, commit, 'apps/garmin-hass/addon/config.yaml')))


def admit_source(source: Path, document: Assembly) -> None:
    commit = clean_commit(source)
    if git(source, 'rev-parse', '--abbrev-ref', 'HEAD') != 'HEAD':
        raise ValueError('Release source must be checked out at the detached tag')
    expected = document['source']
    tagged = git(source, 'rev-parse', '--verify', 'refs/tags/' + expected['tag'] + '^{commit}')
    if commit != expected['commit'] or tagged != commit:
        raise ValueError('Source commit, checkout, and version tag differ')
    branch = 'refs/remotes/origin/main'
    if subprocess.run(
        ['git', '-C', str(source), 'show-ref', '--verify', '--quiet', branch], check=False, capture_output=True
    ).returncode:
        branch = 'refs/heads/main'
    if subprocess.run(
        ['git', '-C', str(source), 'merge-base', '--is-ancestor', commit, branch], check=False, capture_output=True
    ).returncode:
        raise ValueError('Release tag is not reachable from main; fetch its history first')
    version, app = source_metadata(source, commit)
    if version != document['version'] or app.get('version') != version:
        raise ValueError('Workspace, app metadata, and release versions differ')
    if app.get('image') != document['image']['repository']:
        raise ValueError('App metadata and release image repositories differ')
    if not set(platform['architecture'] for platform in document['platforms']).issubset(strings(app.get('arch'))):
        raise ValueError('Release platform is absent from app metadata')
    if hashlib.sha256(source_bytes(source, commit, 'flake.lock')).hexdigest() != expected['lock_sha256']:
        raise ValueError('Source lockfile hash differs from the build receipt')
    if git(source, 'ls-tree', '--name-only', commit, '--', '.gitmodules'):
        raise ValueError('Source archives with submodules need an explicit source-closure contract')


def archive_source(source: Path, document: Assembly, destination: Path) -> None:
    subprocess.run(
        [
            'git',
            '-C',
            str(source),
            'archive',
            '--format=tar.gz',
            '--prefix=garmin-toolkit-' + document['version'] + '/',
            '--output=' + str(destination),
            document['source']['commit'],
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=120,
    )
