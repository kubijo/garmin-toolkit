"""Validate Rust activity exports using the pinned official Frictionless implementation."""

import json
import os
import subprocess
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

import tyro
from frictionless import Package

from cli_output import TerminalOutput


@dataclass(frozen=True)
class Options:
    directory: Path
    reader: Path | None = None


def validate(path: Path) -> None:
    package = Package(str(path))
    report = package.validate()
    if not report.valid:
        raise ValueError(f'{path}: {report}')


def check_reader(path: Path, reader: Path) -> None:
    """Read metadata rewritten by the official library, in a standard deflated ZIP."""
    package = Package(str(path))
    descriptor = json.dumps(package.to_descriptor(), allow_nan=False).encode()
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory) / 'official.zip'
        with (
            zipfile.ZipFile(path) as original,
            zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as rewritten,
        ):
            for member in original.namelist():
                rewritten.writestr(member, descriptor if member == 'datapackage.json' else original.read(member))
        environment = dict(os.environ, GARMIN_TOOLKIT_STATE_DIR=str(Path(directory) / 'state'))
        subprocess.run(
            [str(reader.resolve()), '--json', 'activity', 'inspect', str(output)], env=environment, check=True
        )


def main(options: Options) -> int:
    output = TerminalOutput()
    output.install_tracebacks()
    packages = sorted(options.directory.glob('*.zip'))
    if not packages:
        output.panel('No Rust-generated activity packages found.', title='Export conformance failed', error=True)
        return 1
    try:
        for package in packages:
            validate(package)
            if options.reader is not None:
                check_reader(package, options.reader)
    except (ValueError, subprocess.CalledProcessError) as error:
        output.panel(str(error), title='Export conformance failed', error=True)
        return 1
    output.panel(f'{len(packages)} Rust package(s) passed official validation.', title='Export conformance')
    return 0


if __name__ == '__main__':
    raise SystemExit(main(tyro.cli(Options)))
