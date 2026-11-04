"""Report and enforce LCOV line coverage by Cargo owner."""

import json
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

import tyro
from rich.table import Table

from cli_output import TerminalOutput
from json_data import object_value, strings, text

ROOT = Path(__file__).resolve().parents[2]


@dataclass(frozen=True)
class Counts:
    covered: int = 0
    total: int = 0

    @property
    def percent(self) -> float | None:
        return self.covered * 100 / self.total if self.total else None

    def __add__(self, other: Counts) -> Counts:
        return Counts(self.covered + other.covered, self.total + other.total)


@dataclass(frozen=True)
class FileCoverage:
    path: Path
    lines: Counts


@dataclass(frozen=True)
class Owner:
    name: str
    path: Path


@dataclass(frozen=True)
class OwnerCoverage:
    owner: Owner
    files: list[FileCoverage]

    @property
    def lines(self) -> Counts:
        return sum((file.lines for file in self.files), Counts())


@dataclass(frozen=True)
class Report:
    owners: list[OwnerCoverage]
    unassigned: list[FileCoverage]
    external: list[FileCoverage]


@dataclass(frozen=True)
class Options:
    report: tyro.conf.Positional[Path]
    """Existing LCOV report, such as .tmp/coverage/lcov.info."""
    root: Path = ROOT
    output: Path | None = None
    """Optional machine-readable report."""
    policy: Path | None = None
    """Enforce the reviewed owner floors in this policy."""


@dataclass(frozen=True)
class Policy:
    floors: dict[str, int]
    unmeasured: dict[str, str]


def load_policy(path: Path) -> Policy:
    data = object_value(tomllib.loads(path.read_text()))
    if data.get('version') != 1 or set(data) != {'version', 'measurement', 'floors', 'unmeasured'}:
        raise ValueError('Unsupported coverage policy structure')
    floors: dict[str, int] = {}
    for name, value in object_value(data['floors']).items():
        if type(value) is not int or not 0 <= value <= 100:
            raise ValueError(f'Invalid coverage floor for {name}: {value}')
        floors[name] = value
    unmeasured = {name: text(reason) for name, reason in object_value(data['unmeasured']).items()}
    if any(not reason.strip() for reason in unmeasured.values()) or floors.keys() & unmeasured.keys():
        raise ValueError('Unmeasured owners require a reason and cannot also have a floor')
    return Policy(floors, unmeasured)


def enforce(report: Report, policy: Policy) -> None:
    errors: list[str] = []
    owners = {row.owner.name for row in report.owners}
    configured = policy.floors.keys() | policy.unmeasured.keys()
    for name in sorted(owners - configured):
        errors.append(f'{name}: missing owner policy')
    for name in sorted(configured - owners):
        errors.append(f'{name}: stale owner policy')
    for row in report.owners:
        name, lines = row.owner.name, row.lines
        if name in policy.floors:
            floor = policy.floors[name]
            if not lines.total or lines.covered * 100 < floor * lines.total:
                errors.append(f'{name}: {lines.covered}/{lines.total} covered lines, requires {floor}%')
        elif name in policy.unmeasured and lines.total:
            errors.append(f'{name}: now measured; replace its exemption with a floor')
    if report.unassigned:
        errors.append('Workspace files without an owner: ' + ', '.join(str(file.path) for file in report.unassigned))
    if errors:
        raise ValueError('\n'.join(errors))


def parse_lcov(source: str, root: Path) -> list[FileCoverage]:
    result: list[FileCoverage] = []
    seen: set[Path] = set()
    path: Path | None = None
    counts: dict[str, int] = {}
    for line in source.splitlines():
        if line.startswith('SF:'):
            if path is not None:
                raise ValueError('LCOV file record is missing end_of_record')
            value = line.removeprefix('SF:')
            if not value:
                raise ValueError('LCOV file record has an empty path')
            path = (root / value).resolve()
            if path in seen:
                raise ValueError(f'Duplicate LCOV file record: {path}')
        elif line.startswith(('LF:', 'LH:')):
            key, value = line.split(':', 1)
            if path is None or key in counts or not value.isascii() or not value.isdecimal():
                raise ValueError(f'Invalid LCOV line count: {line}')
            counts[key] = int(value)
        elif line == 'end_of_record':
            if path is None or counts.keys() != {'LF', 'LH'} or counts['LH'] > counts['LF']:
                raise ValueError('LCOV file record has missing or inconsistent line counts')
            result.append(FileCoverage(path, Counts(counts['LH'], counts['LF'])))
            seen.add(path)
            path = None
            counts = {}
    if path is not None or not result:
        raise ValueError('Incomplete or empty LCOV report')
    return result


def workspace_owners(root: Path) -> list[Owner]:
    manifest = object_value(tomllib.loads((root / 'Cargo.toml').read_text()))
    workspace = object_value(manifest['workspace'])
    excluded = {path.resolve() for pattern in strings(workspace.get('exclude', [])) for path in root.glob(pattern)}
    owners: dict[Path, Owner] = {}
    names: set[str] = set()
    for pattern in strings(workspace['members']):
        for directory in root.glob(pattern):
            path = directory.resolve()
            if path in excluded or path in owners:
                continue
            package = object_value(tomllib.loads((path / 'Cargo.toml').read_text())['package'])
            name = text(package['name'])
            if name in names:
                raise ValueError(f'Duplicate Cargo owner: {name}')
            names.add(name)
            owners[path] = Owner(name, path)
    if not owners:
        raise ValueError('Workspace has no Cargo owners')
    return sorted(owners.values(), key=lambda owner: len(owner.path.parts), reverse=True)


def summarize(files: list[FileCoverage], owners: list[Owner], root: Path) -> Report:
    rows = [OwnerCoverage(owner, []) for owner in sorted(owners, key=lambda owner: len(owner.path.parts), reverse=True)]
    unassigned: list[FileCoverage] = []
    external: list[FileCoverage] = []
    for file in files:
        if not file.path.is_relative_to(root):
            external.append(file)
            continue
        owner = next((row for row in rows if file.path.is_relative_to(row.owner.path)), None)
        if owner is None:
            unassigned.append(file)
        else:
            owner.files.append(file)
    return Report(
        sorted(rows, key=lambda row: (row.lines.percent is None, row.lines.percent or 0, row.owner.name)),
        unassigned,
        external,
    )


def file_data(file: FileCoverage, root: Path) -> dict[str, object]:
    return {
        'path': str(file.path.relative_to(root)) if file.path.is_relative_to(root) else str(file.path),
        'covered': file.lines.covered,
        'total': file.lines.total,
    }


def report_data(report: Report, root: Path) -> dict[str, object]:
    return {
        'schema_version': 1,
        'owners': [
            {
                'owner': row.owner.name,
                'path': str(row.owner.path.relative_to(root)),
                'covered': row.lines.covered,
                'total': row.lines.total,
                'percent': row.lines.percent,
                'files': [file_data(file, root) for file in row.files],
            }
            for row in report.owners
        ],
        'unassigned': [file_data(file, root) for file in report.unassigned],
        'external': [file_data(file, root) for file in report.external],
    }


def run(options: Options, output: TerminalOutput) -> None:
    root = options.root.resolve()
    report = summarize(parse_lcov(options.report.read_text(), root), workspace_owners(root), root)
    table = Table('Cargo owner', 'Files', 'Covered lines', 'Coverage', title='Coverage by owner')
    for row in report.owners:
        lines = row.lines
        table.add_row(
            row.owner.name,
            str(len(row.files)),
            f'{lines.covered}/{lines.total}',
            f'{lines.percent:.2f}%' if lines.percent is not None else 'unmeasured',
        )
    output.console.print(table)
    for label, files in [('Unassigned workspace files', report.unassigned), ('External files', report.external)]:
        if files:
            output.panel('\n'.join(str(file.path) for file in files), title=label)
    if options.output is not None:
        options.output.parent.mkdir(parents=True, exist_ok=True)
        options.output.write_text(json.dumps(report_data(report, root), indent=2) + '\n')
    if options.policy is not None:
        enforce(report, load_policy(options.policy))


def main() -> None:
    output = TerminalOutput()
    output.install_tracebacks()
    try:
        run(tyro.cli(Options), output)
    except (OSError, ValueError, KeyError) as error:
        output.panel(str(error), title='Coverage report failed', style='red', error=True)
        sys.exit(1)


if __name__ == '__main__':
    main()
