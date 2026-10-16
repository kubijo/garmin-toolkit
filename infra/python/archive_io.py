"""Bounded streaming image reads without extracting archive contents."""

import gzip
import io
import stat
import tarfile
from collections.abc import Generator, Iterator
from contextlib import closing, contextmanager, nullcontext
from dataclasses import dataclass
from pathlib import Path
from typing import IO

MAX_IMAGE_BYTES = 4 * 1024**3
MAX_MEMBERS = 100_000
MAX_METADATA_BYTES = 1024 * 1024


def check_archive(path: Path) -> None:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= MAX_IMAGE_BYTES:
        raise ValueError('Expected a nonempty regular image within the archive size limit')


class LimitedReader(io.RawIOBase):
    def __init__(self, source: IO[bytes] | gzip.GzipFile, limit: int) -> None:
        super().__init__()
        self.source = source
        self.remaining = limit
        self.header_remaining: int | None = None

    def read(self, size: int | None = -1, /) -> bytes:
        if size is None or size < 0:
            raise ValueError('Image inspection requires bounded reads')
        size = min(size, self.remaining + 1)
        if self.header_remaining is not None:
            size = min(size, self.header_remaining + 1)
        data = self.source.read(size)
        self.remaining -= len(data)
        if self.remaining < 0:
            raise ValueError('Image exceeds the uncompressed archive size limit')
        if self.header_remaining is not None:
            self.header_remaining -= len(data)
            if self.header_remaining < 0:
                raise ValueError('Image contains oversized tar extension metadata')
        return data


@dataclass
class Archive:
    tar: tarfile.TarFile
    reader: LimitedReader

    def __iter__(self) -> Iterator[tarfile.TarInfo]:
        metadata = 0
        for count, member in enumerate(self.tar, 1):
            metadata += member.offset_data - member.offset
            if count > MAX_MEMBERS or member.offset_data + member.size > MAX_IMAGE_BYTES:
                raise ValueError('Image exceeds the archive member or uncompressed size limit')
            if metadata > 128 * MAX_METADATA_BYTES or member.issparse():
                raise ValueError('Image has excessive metadata or unsupported sparse members')
            self.reader.header_remaining = None
            yield member
            # Skip unread payload before budgeting the next header, including GNU/PAX extension reads.
            self.tar.fileobj.seek(self.tar.offset)
            self.reader.header_remaining = MAX_METADATA_BYTES
        self.reader.header_remaining = None
        for _ in chunks(self.reader):
            pass

    def extractfile(self, member: tarfile.TarInfo) -> IO[bytes]:
        stream = self.tar.extractfile(member)
        if stream is None:
            raise ValueError('Expected a regular image member')
        return stream


@contextmanager
def open_tar(source: Path | IO[bytes]) -> Generator[Archive]:
    if isinstance(source, Path):
        check_archive(source)
    with source.open('rb') if isinstance(source, Path) else closing(source) as raw:
        compressed = False
        if isinstance(source, Path):
            compressed = raw.read(2) == b'\x1f\x8b'
            raw.seek(0)
        with (
            gzip.GzipFile(fileobj=raw) if compressed else nullcontext(raw) as decoded,
            LimitedReader(decoded, MAX_IMAGE_BYTES) as limited,
        ):
            limited.header_remaining = MAX_METADATA_BYTES
            with tarfile.open(fileobj=limited, mode='r|') as archive:
                archive.stream = True
                yield Archive(archive, limited)


def chunks(source: io.Reader[bytes]) -> Iterator[bytes]:
    while data := source.read(1024 * 1024):
        yield data


def expand(archive: Path, destination: Path) -> Path:
    check_archive(archive)
    with archive.open('rb') as original:
        if original.read(2) != b'\x1f\x8b':
            return archive
    with (
        gzip.open(archive, 'rb') as source,
        LimitedReader(source, MAX_IMAGE_BYTES) as limited,
        destination.open('xb') as target,
    ):
        for chunk in chunks(limited):
            target.write(chunk)
    return destination
