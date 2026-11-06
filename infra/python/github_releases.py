"""Typed GitHub release operations; only this adapter knows the gh command layout."""

import json
import re
import subprocess
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from json_data import object_rows, object_value, text


@dataclass(frozen=True)
class ReleaseAsset:
    name: str
    digest: str | None


class Releases(Protocol):
    def tag_commit(self, tag: str) -> str: ...
    def is_draft(self, tag: str) -> bool: ...
    def assets(self, tag: str) -> list[ReleaseAsset]: ...
    def download(self, tag: str, destination: Path, *, pattern: str | None = None) -> None: ...
    def upload(self, tag: str, paths: Sequence[Path]) -> None: ...


class Command(Protocol):
    def __call__(self, *arguments: str) -> str: ...


def command(*arguments: str) -> str:
    return subprocess.run(arguments, check=True, text=True, stdout=subprocess.PIPE).stdout.rstrip('\n')


class GitHubReleases(Releases):
    def __init__(self, repository: str, run: Command = command) -> None:
        self.repository = repository
        self.run = run

    def tag_commit(self, tag: str) -> str:
        reference = f'refs/tags/{tag}'
        rows = self.run(
            'git', 'ls-remote', '--exit-code', f'https://github.com/{self.repository}.git', reference, reference + '^{}'
        )
        commits: dict[str, str] = {}
        for row in rows.splitlines():
            commit, name = row.split('\t', 1)
            if name not in (reference, reference + '^{}') or not re.fullmatch(r'[0-9a-f]{40}', commit):
                raise ValueError('Remote tag returned an invalid Git reference')
            if name in commits:
                raise ValueError('Remote tag returned duplicate Git references')
            commits[name] = commit
        commit = commits.get(reference + '^{}', commits.get(reference))
        if commit is None:
            raise ValueError('Release tag does not exist remotely')
        return commit

    def is_draft(self, tag: str) -> bool:
        document = object_value(
            json.loads(
                self.run(
                    'gh',
                    'release',
                    'view',
                    tag,
                    '--repo',
                    self.repository,
                    '--json',
                    'isDraft',
                )
            )
        )
        value = document.get('isDraft')
        if not isinstance(value, bool):
            raise ValueError('Release has no boolean draft state')
        return value

    def assets(self, tag: str) -> list[ReleaseAsset]:
        document = object_value(json.loads(self.run('gh', 'api', f'repos/{self.repository}/releases/tags/{tag}')))
        result: list[ReleaseAsset] = []
        for row in object_rows(document['assets']):
            digest = row.get('digest')
            if digest is not None and not isinstance(digest, str):
                raise ValueError('Release asset digest must be a string or null')
            result.append(ReleaseAsset(text(row['name']), digest or None))
        if len({asset.name for asset in result}) != len(result):
            raise ValueError('Release has duplicate asset names')
        return result

    def download(self, tag: str, destination: Path, *, pattern: str | None = None) -> None:
        options = ('--pattern', pattern) if pattern is not None else ()
        self.run('gh', 'release', 'download', tag, '--repo', self.repository, '--dir', str(destination), *options)

    def upload(self, tag: str, paths: Sequence[Path]) -> None:
        self.run('gh', 'release', 'upload', tag, '--repo', self.repository, *map(str, paths))
