"""In-memory release state and local asset downloads for publication tests."""

import fnmatch
import hashlib
import shutil
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path

from github_releases import ReleaseAsset, Releases

FIXTURE_COMMIT = '1' * 40


@dataclass(frozen=True)
class Download:
    tag: str
    destination: Path
    pattern: str | None


@dataclass(frozen=True)
class Upload:
    tag: str
    paths: tuple[Path, ...]


@dataclass
class ReleaseFixture(Releases):
    directory: Path
    draft: bool = True
    commit: str = FIXTURE_COMMIT
    existing_assets: list[ReleaseAsset] = field(default_factory=list)
    lookups: list[str] = field(default_factory=list)
    downloads: list[Download] = field(default_factory=list)
    uploads: list[Upload] = field(default_factory=list)
    tag_lookups: list[str] = field(default_factory=list)

    def tag_commit(self, tag: str) -> str:
        self.tag_lookups.append(tag)
        return self.commit

    def is_draft(self, tag: str) -> bool:
        self.lookups.append(tag)
        return self.draft

    def assets(self, tag: str) -> list[ReleaseAsset]:
        self.lookups.append(tag)
        return list(self.existing_assets)

    def download(self, tag: str, destination: Path, *, pattern: str | None = None) -> None:
        self.downloads.append(Download(tag, destination, pattern))
        for path in self.directory.iterdir():
            if pattern is None or fnmatch.fnmatchcase(path.name, pattern):
                shutil.copyfile(path, destination / path.name)

    def upload(self, tag: str, paths: Sequence[Path]) -> None:
        if {path.name for path in paths} & {asset.name for asset in self.existing_assets}:
            raise FileExistsError('Published assets cannot be overwritten')
        self.uploads.append(Upload(tag, tuple(paths)))
        for path in paths:
            with path.open('rb') as source:
                digest = hashlib.file_digest(source, 'sha256').hexdigest()
            self.existing_assets.append(ReleaseAsset(path.name, 'sha256:' + digest))
