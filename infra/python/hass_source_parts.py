"""Stream one gzip archive across bounded files without joining it on disk."""

import hashlib
import io
from collections.abc import Buffer
from pathlib import Path

MAX_PARTS = 16


def part_path(base: Path, number: int) -> Path:
    return base if number == 1 else base.with_name(base.name + f'.{number:03}')


class PartsWriter(io.RawIOBase):
    def __init__(self, base: Path, limit: int) -> None:
        super().__init__()
        self.base, self.limit = base, limit
        self.paths: list[Path] = []
        self.current: io.FileIO | None = None
        self.position = 0
        self.digest = hashlib.sha256()
        self.failed = False

    def writable(self) -> bool:
        return True

    def tell(self) -> int:
        return self.position

    def write(self, buffer: Buffer, /) -> int:
        data = memoryview(buffer).cast('B')
        length = len(data)
        if self.failed:
            return length  # Let gzip/tar finalizers close without masking the original failure.
        try:
            while data:
                if self.current is None or self.current.tell() == self.limit:
                    if self.current is not None:
                        self.current.close()
                    if len(self.paths) == MAX_PARTS:
                        raise ValueError('Dependency source bundle exceeds the part count limit')
                    path = part_path(self.base, len(self.paths) + 1)
                    self.current = io.FileIO(path, 'x')
                    self.paths.append(path)
                count = self.current.write(data[: self.limit - self.current.tell()])
                if count is None or count <= 0:
                    raise OSError('Could not write dependency source part')
                self.digest.update(data[:count])
                self.position += count
                data = data[count:]
        except OSError, ValueError:
            self.failed = True
            raise
        return length

    def close(self) -> None:
        try:
            if self.current is not None:
                self.current.close()
        finally:
            super().close()


class PartsReader(io.RawIOBase):
    def __init__(self, paths: list[Path]) -> None:
        super().__init__()
        self.paths = iter(paths)
        self.current: io.FileIO | None = None

    def readable(self) -> bool:
        return True

    def readinto(self, buffer: Buffer, /) -> int:
        while True:
            if self.current is None:
                path = next(self.paths, None)
                if path is None:
                    return 0
                self.current = io.FileIO(path, 'r')
            count = self.current.readinto(buffer)
            if count:
                return count
            self.current.close()
            self.current = None

    def close(self) -> None:
        try:
            if self.current is not None:
                self.current.close()
        finally:
            super().close()
