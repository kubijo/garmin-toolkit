"""Owned child processes for the build profiler's descriptor and signal tests."""

import os
import sys
import time
from pathlib import Path
from typing import Literal

import tyro

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from build_profile import invoke


def main(
    mode: Literal['descriptors', 'descriptor-child', 'signal', 'signal-child'],
    read_fd: int = 0,
    write_fd: int = 0,
    ready: Path | None = None,
) -> int:
    if mode == 'descriptor-child':
        os.fstat(read_fd)
        os.fstat(write_fd)
        return 0
    if mode == 'signal-child':
        assert ready is not None
        ready.touch()
        time.sleep(60)
        return 0
    command = [sys.executable, str(Path(__file__).resolve())]
    if mode == 'descriptors':
        command.extend(['descriptor-child', '--read-fd', str(read_fd), '--write-fd', str(write_fd)])
    else:
        assert ready is not None
        command.extend(['signal-child', '--ready', str(ready)])
    return invoke(command, 'fixture', group=True)


if __name__ == '__main__':
    sys.exit(tyro.cli(main, config=(tyro.conf.PositionalRequiredArgs,)))
