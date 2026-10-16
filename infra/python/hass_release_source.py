"""Read-only source admission for a tagged HASS release."""

import hashlib
import subprocess
import tomllib
from pathlib import Path
from typing import Any, cast

import yaml

from hass_release_contract import Assembly


def git(source: Path, *arguments: str) -> str:
    return subprocess.check_output(['git', '-C', str(source), *arguments], text=True, stderr=subprocess.PIPE).strip()


def source_bytes(source: Path, document: Assembly, name: str) -> bytes:
    """Use committed blobs even when a checkout has ignored or assume-unchanged files."""
    commit = document['source']['commit']
    entry = git(source, 'ls-tree', commit, '--', name)
    if not entry.startswith(('100644 blob ', '100755 blob ')):
        raise ValueError(f'Expected a tracked regular source file: {name}')
    return subprocess.check_output(['git', '-C', str(source), 'show', commit + ':' + name])


def admit_source(source: Path, document: Assembly) -> None:
    source = source.resolve()
    if Path(git(source, 'rev-parse', '--show-toplevel')).resolve() != source:
        raise ValueError('Source must be the repository root')
    if git(source, 'status', '--porcelain=v1', '--untracked-files=all'):
        raise ValueError('Release source tree is dirty')
    if git(source, 'rev-parse', '--abbrev-ref', 'HEAD') != 'HEAD':
        raise ValueError('Release source must be checked out at the detached tag')
    expected = document['source']
    commit = git(source, 'rev-parse', '--verify', 'HEAD')
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
    version = tomllib.loads(source_bytes(source, document, 'Cargo.toml').decode())['workspace']['package']['version']
    app = cast(dict[str, Any], yaml.safe_load(source_bytes(source, document, 'apps/garmin-hass/addon/config.yaml')))
    if version != document['version'] or app.get('version') != version:
        raise ValueError('Workspace, app metadata, and release versions differ')
    if app.get('image') != document['image']['repository']:
        raise ValueError('App metadata and release image repositories differ')
    if not set(platform['architecture'] for platform in document['platforms']).issubset(app['arch']):
        raise ValueError('Release platform is absent from app metadata')
    if hashlib.sha256(source_bytes(source, document, 'flake.lock')).hexdigest() != expected['lock_sha256']:
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
