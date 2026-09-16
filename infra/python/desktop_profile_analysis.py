"""Headless analysis and comparison for guarded desktop profiling reports."""

import argparse
import gzip
import json
import math
import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from desktop_profile import (
    COMBINED_NAME,
    MANIFEST_NAME,
    MANIFEST_SCHEMA_VERSION,
    METRICS_NAME,
    REPORTS_ROOT,
    ProfileError,
    file_digest,
    load_metrics,
)

INTERACTION_FRAME_TARGET_MILLISECONDS = 1_000.0 / 60.0
INTERACTION_STALL_LIMIT_MILLISECONDS = 33.0
MINIMUM_INTERACTION_MILLISECONDS = 2_000.0
MINIMUM_INTERACTION_SAMPLES = 120


@dataclass(frozen=True)
class Distribution:
    count: int
    mean: float
    p50: float
    p95: float
    p99: float
    maximum: float

    @classmethod
    def from_values(cls, values: list[float]) -> Distribution | None:
        if not values:
            return None
        ordered = sorted(values)

        def percentile(value: float) -> float:
            rank = max(1, math.ceil(len(ordered) * value))
            return ordered[min(len(ordered) - 1, rank - 1)]

        return cls(
            count=len(ordered),
            mean=sum(ordered) / len(ordered),
            p50=percentile(0.50),
            p95=percentile(0.95),
            p99=percentile(0.99),
            maximum=ordered[-1],
        )


@dataclass(frozen=True)
class RuntimeSummary:
    """Comparable runtime measurements derived from immutable NDJSON evidence."""

    duration_seconds: float
    interaction_frame: Distribution | None
    interaction_desktop_ui: Distribution | None
    interaction_map_ui: Distribution | None
    redraw_frame: Distribution | None
    map_ui: Distribution | None
    desktop_ui: Distribution | None
    scene: Distribution | None
    route_query: Distribution | None
    render_prepare: Distribution | None
    render_draw: Distribution | None
    max_pending_tiles: int
    max_label_backlog: int
    stale_work: int
    metrics_schema_version: int


@dataclass(frozen=True)
class CpuSummary:
    sampled_cpu_seconds: float
    main_samples: int
    total_samples: int
    inclusive: Counter[str]
    exclusive: Counter[str]
    crates: Counter[str]


@dataclass(frozen=True)
class ReportAnalysis:
    directory: Path
    manifest: dict[str, Any]
    runtime: RuntimeSummary
    cpu: CpuSummary


class SymbolResolver:
    """Resolve processed-profile frames through Samply's presymbolication sidecar."""

    def __init__(self, profile: dict[str, Any], sidecar: dict[str, Any] | None) -> None:
        self.profile = profile
        self.addresses: dict[str, dict[int, tuple[str, ...]]] = {}
        if sidecar is None:
            return
        strings = sidecar.get('string_table')
        images = sidecar.get('data')
        if not isinstance(strings, list) or not isinstance(images, list):
            raise ProfileError('Samply symbol sidecar has an unexpected shape')
        for image in images:
            code_id = image.get('code_id')
            symbols = image.get('symbol_table')
            known = image.get('known_addresses')
            if (
                not isinstance(code_id, str)
                or not code_id
                or not isinstance(symbols, list)
                or not isinstance(known, list)
            ):
                continue
            resolved: dict[int, tuple[str, ...]] = {}
            for entry in known:
                if not isinstance(entry, list) or len(entry) != 2 or not all(isinstance(value, int) for value in entry):
                    raise ProfileError('Samply symbol sidecar contains an invalid known address')
                address, symbol_index = entry
                if not 0 <= symbol_index < len(symbols):
                    raise ProfileError('Samply symbol sidecar contains an invalid symbol index')
                symbol = symbols[symbol_index]
                frames = symbol.get('frames') or []
                names = []
                for frame in frames:
                    name_index = frame.get('function')
                    if isinstance(name_index, int) and 0 <= name_index < len(strings):
                        name = strings[name_index]
                        if isinstance(name, str):
                            names.append(name)
                if not names:
                    name_index = symbol.get('symbol')
                    if isinstance(name_index, int) and 0 <= name_index < len(strings):
                        name = strings[name_index]
                        if isinstance(name, str):
                            names.append(name)
                if names:
                    # Samply records inline frames from the sampled innermost function outwards.
                    resolved[address] = tuple(names)
            self.addresses[code_id] = resolved

    def frames(self, thread: dict[str, Any], frame_index: int) -> tuple[str, ...]:
        frame_table = thread['frameTable']
        function_table = thread['funcTable']
        function_index = frame_table['func'][frame_index]
        raw = thread['stringArray'][function_table['name'][function_index]]
        resource_index = function_table['resource'][function_index]
        if resource_index is None or resource_index < 0:
            return (raw,)
        library_index = thread['resourceTable']['lib'][resource_index]
        if library_index is None or library_index < 0:
            return (raw,)
        library = self.profile['libs'][library_index]
        code_id = library.get('codeId')
        address = frame_table['address'][frame_index]
        return self.addresses.get(code_id, {}).get(address, (raw,))


def report_directory(value: str) -> Path:
    candidate = Path(value)
    if candidate.is_dir():
        return candidate.resolve()
    return (REPORTS_ROOT / value).resolve()


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ProfileError(f'{path} has an unexpected root value')
    return value


def load_profile(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise ProfileError(f'profile is missing: {path}')
    with gzip.open(path, 'rt') as source:
        value = json.load(source)
    if not isinstance(value, dict):
        raise ProfileError(f'{path} has an unexpected root value')
    return value


def distribution(rows: list[dict[str, Any]], key: str) -> Distribution | None:
    values = [
        float(row[key]) for row in rows if isinstance(row.get(key), int | float) and not isinstance(row[key], bool)
    ]
    return Distribution.from_values(values)


def measured_interaction(row: dict[str, Any]) -> bool:
    value = row.get('interaction_frame_milliseconds')
    return isinstance(value, int | float) and not isinstance(value, bool)


def runtime_summary(report: Path) -> RuntimeSummary:
    header, rows = load_metrics(report / METRICS_NAME)
    map_frames = [row for row in rows if row['kind'] == 'map_frame']
    desktop_frames = [row for row in rows if row['kind'] == 'desktop_frame']
    interaction_map_frames = [row for row in map_frames if measured_interaction(row)]
    interaction_desktop_frames: list[dict[str, Any]] = []
    preceding_map_measured_interaction = False
    for row in rows:
        if row['kind'] == 'map_frame':
            preceding_map_measured_interaction = measured_interaction(row)
        elif row['kind'] == 'desktop_frame':
            if preceding_map_measured_interaction:
                interaction_desktop_frames.append(row)
            preceding_map_measured_interaction = False
    prepare = [row for row in rows if row['kind'] == 'map_render' and row['phase'] == 'prepare']
    draw = [row for row in rows if row['kind'] == 'map_render' and row['phase'] == 'draw']
    duration = max(float(row['elapsed_milliseconds']) for row in rows) / 1_000.0
    return RuntimeSummary(
        duration_seconds=duration,
        interaction_frame=distribution(map_frames, 'interaction_frame_milliseconds'),
        interaction_desktop_ui=distribution(interaction_desktop_frames, 'ui_milliseconds'),
        interaction_map_ui=distribution(interaction_map_frames, 'ui_milliseconds'),
        redraw_frame=distribution(map_frames, 'frame_milliseconds'),
        map_ui=distribution(map_frames, 'ui_milliseconds'),
        desktop_ui=distribution(desktop_frames, 'ui_milliseconds'),
        scene=distribution(map_frames, 'scene_milliseconds'),
        route_query=distribution(map_frames, 'route_query_microseconds'),
        render_prepare=distribution(prepare, 'milliseconds'),
        render_draw=distribution(draw, 'milliseconds'),
        max_pending_tiles=max((int(row['pending_tiles']) for row in map_frames), default=0),
        max_label_backlog=max((int(row['label_backlog']) for row in map_frames), default=0),
        stale_work=max((int(row['stale_work']) for row in map_frames), default=0),
        metrics_schema_version=int(header['schema_version']),
    )


def main_thread(profile: dict[str, Any]) -> dict[str, Any]:
    threads = profile.get('threads')
    if not isinstance(threads, list):
        raise ProfileError('Samply profile contains no thread table')
    candidates = [
        thread
        for thread in threads
        if isinstance(thread, dict)
        and thread.get('isMainThread') is True
        and thread.get('processName') == 'garmin-desktop'
    ]
    if not candidates:
        candidates = [
            thread
            for thread in threads
            if isinstance(thread, dict) and thread.get('name') in {'main', 'garmin-desktop'}
        ]
    if not candidates:
        raise ProfileError('Samply profile contains no desktop UI thread')
    return max(candidates, key=lambda thread: int(thread['samples']['length']))


def extract_crate(symbol: str) -> str:
    if symbol.startswith('0x'):
        return '[unsymbolized]'
    match = re.match(r'<?([A-Za-z_][A-Za-z0-9_]*)(?:::|_ir::)', symbol)
    if match:
        return match.group(1)
    if '::' not in symbol and '<' not in symbol:
        return '[system]'
    return '[other]'


def stack_summary(thread: dict[str, Any], resolver: SymbolResolver) -> tuple[Counter[str], Counter[str], Counter[str]]:
    """Compute self and inclusive sample counts, deduplicated per stack."""
    stack_counts = Counter(thread['samples']['stack'])
    stacks = thread['stackTable']
    exclusive: Counter[str] = Counter()
    inclusive: Counter[str] = Counter()
    crates: Counter[str] = Counter()
    for stack_index, count in stack_counts.items():
        if stack_index is None:
            continue
        frame_index = stacks['frame'][stack_index]
        exclusive[resolver.frames(thread, frame_index)[0]] += count
        seen_symbols: set[str] = set()
        seen_crates: set[str] = set()
        while stack_index is not None:
            frame_index = stacks['frame'][stack_index]
            for symbol in resolver.frames(thread, frame_index):
                if symbol not in seen_symbols:
                    inclusive[symbol] += count
                    seen_symbols.add(symbol)
                crate = extract_crate(symbol)
                if crate not in seen_crates:
                    crates[crate] += count
                    seen_crates.add(crate)
            stack_index = stacks['prefix'][stack_index]
    return exclusive, inclusive, crates


def cpu_summary(report: Path, profile: dict[str, Any]) -> CpuSummary:
    sidecar_path = report / 'combined.json.syms.json'
    if not sidecar_path.is_file():
        sidecar_path = report / 'profile.json.syms.json'
    sidecar = load_json(sidecar_path) if sidecar_path.is_file() else None
    resolver = SymbolResolver(profile, sidecar)
    thread = main_thread(profile)
    exclusive, inclusive, crates = stack_summary(thread, resolver)
    process_id = thread.get('pid')
    threads = [candidate for candidate in profile['threads'] if candidate.get('pid') == process_id]
    total_samples = sum(int(candidate['samples']['length']) for candidate in threads)
    cpu_microseconds = sum(
        float(delta or 0.0) for candidate in threads for delta in candidate['samples'].get('threadCPUDelta', [])
    )
    return CpuSummary(
        sampled_cpu_seconds=cpu_microseconds / 1_000_000.0,
        main_samples=int(thread['samples']['length']),
        total_samples=total_samples,
        inclusive=inclusive,
        exclusive=exclusive,
        crates=crates,
    )


def verify_artifacts(report: Path, manifest: dict[str, Any]) -> None:
    """Verify every immutable report artifact against its completed manifest."""
    if manifest.get('schema_version') != MANIFEST_SCHEMA_VERSION:
        raise ProfileError(f'{report} uses an unsupported profiling manifest schema')
    artifacts = manifest.get('artifacts')
    if not isinstance(artifacts, dict):
        raise ProfileError(f'{report} has no profiling artifact ledger')
    required = {COMBINED_NAME, METRICS_NAME}
    if not required.issubset(artifacts):
        missing = ', '.join(sorted(required.difference(artifacts)))
        raise ProfileError(f'{report} does not account for required artifacts: {missing}')
    for name, expected in artifacts.items():
        if not isinstance(name, str) or Path(name).name != name or not isinstance(expected, dict):
            raise ProfileError(f'{report} contains an invalid artifact ledger entry')
        path = report / name
        if not path.is_file() or path.is_symlink():
            raise ProfileError(f'profile artifact is missing or unsafe: {path}')
        if expected.get('bytes') != path.stat().st_size or expected.get('sha256') != file_digest(path):
            raise ProfileError(f'profile artifact does not match its manifest: {path}')


def analyze_report(report: Path) -> ReportAnalysis:
    manifest_path = report / MANIFEST_NAME
    if not manifest_path.is_file():
        raise ProfileError(f'profiling manifest is missing: {manifest_path}')
    manifest = load_json(manifest_path)
    if manifest.get('status') != 'complete':
        raise ProfileError(f'{report} is not a complete profiling report')
    verify_artifacts(report, manifest)
    profile = load_profile(report / COMBINED_NAME)
    return ReportAnalysis(report, manifest, runtime_summary(report), cpu_summary(report, profile))


def format_distribution(value: Distribution | None, unit: str) -> str:
    if value is None:
        return 'unavailable'
    return (
        f'n {value.count}  p50 {value.p50:.3f} {unit}  p95 {value.p95:.3f} {unit}  '
        f'p99 {value.p99:.3f} {unit}  max {value.maximum:.3f} {unit}'
    )


def interaction_gate(frame: Distribution | None, desktop_ui: Distribution | None) -> str:
    """Evaluate the documented 60 FPS interaction and 33 ms stall limits."""
    if frame is None or desktop_ui is None:
        return 'unavailable'
    active_milliseconds = frame.mean * frame.count
    if frame.count < MINIMUM_INTERACTION_SAMPLES or active_milliseconds < MINIMUM_INTERACTION_MILLISECONDS:
        return f'INSUFFICIENT (n={frame.count}, active={active_milliseconds / 1_000.0:.1f} s)'
    passing = (
        frame.p95 <= INTERACTION_FRAME_TARGET_MILLISECONDS
        and frame.maximum <= INTERACTION_STALL_LIMIT_MILLISECONDS
        and desktop_ui.p95 <= INTERACTION_FRAME_TARGET_MILLISECONDS
    )
    return 'PASS' if passing else 'FAIL'


def print_hot(title: str, values: Counter[str], total: int, limit: int) -> None:
    print(f'\n{title}')
    if total == 0:
        print('  unavailable')
        return
    for symbol, count in values.most_common(limit):
        print(f'{100.0 * count / total:6.2f}%  {count:5d}  {symbol[:120]}')


def print_analysis(analysis: ReportAnalysis) -> None:
    runtime = analysis.runtime
    cpu = analysis.cpu
    print(f'Profile: {analysis.directory}')
    print(f'Duration: {runtime.duration_seconds:.1f} s')
    print('\nRuntime')
    print(f'  Interaction interval  {format_distribution(runtime.interaction_frame, "ms")}')
    print(f'  Interaction UI        {format_distribution(runtime.interaction_desktop_ui, "ms")}')
    print(f'  Interaction map UI    {format_distribution(runtime.interaction_map_ui, "ms")}')
    print(f'  Interaction gate      {interaction_gate(runtime.interaction_frame, runtime.interaction_desktop_ui)}')
    if runtime.metrics_schema_version < 2:
        print('                        capture predates camera-active telemetry; redraw gaps include idle time')
    elif runtime.interaction_frame is None:
        print('                        no sustained camera interaction was captured')
    print(f'  Redraw interval       {format_distribution(runtime.redraw_frame, "ms")}')
    print(f'  Desktop UI            {format_distribution(runtime.desktop_ui, "ms")}')
    print(f'  Map UI                {format_distribution(runtime.map_ui, "ms")}')
    print(f'  Scene acquisition     {format_distribution(runtime.scene, "ms")}')
    print(f'  Route query           {format_distribution(runtime.route_query, "µs")}')
    print(f'  Render prepare        {format_distribution(runtime.render_prepare, "ms")}')
    print(f'  Render draw           {format_distribution(runtime.render_draw, "ms")}')
    print(
        f'  Pressure              pending tiles {runtime.max_pending_tiles}, '
        f'label backlog {runtime.max_label_backlog}, stale work {runtime.stale_work}'
    )
    core_equivalent = cpu.sampled_cpu_seconds / runtime.duration_seconds
    print('\nSampled CPU')
    print(
        f'  {cpu.sampled_cpu_seconds:.2f} CPU-s ({core_equivalent:.2f} core-equivalent), '
        f'{cpu.total_samples} process samples, {cpu.main_samples} UI-thread samples'
    )
    accounted_main_samples = sum(cpu.exclusive.values())
    print_hot('Inclusive crates', cpu.crates, accounted_main_samples, 12)
    print_hot('Inclusive UI-thread functions', cpu.inclusive, accounted_main_samples, 20)
    print_hot('Self UI-thread functions', cpu.exclusive, accounted_main_samples, 15)

    if analysis.manifest.get('git', {}).get('dirty') is True:
        print(
            '\nWarning: capture used a dirty source tree; the binary hash identifies it, '
            'but the source state is not reproducible.'
        )


def comparison_value(summary: RuntimeSummary, key: str) -> float | None:
    if key == 'cpu_cores':
        raise AssertionError('CPU comparison is resolved from the complete analysis')
    distribution_value = getattr(summary, key)
    return None if distribution_value is None else distribution_value.p95


def interaction_seconds(summary: RuntimeSummary) -> float:
    interaction = summary.interaction_frame
    return 0.0 if interaction is None else interaction.mean * interaction.count / 1_000.0


def comparison_cell(value: float | None, previous: float | None, unit: str) -> str:
    if value is None:
        return '—'
    rendered = f'{value:.3f}{unit}'
    if previous in {None, 0.0}:
        return rendered
    return f'{rendered} ({(value - previous) * 100.0 / previous:+.1f}%)'


def record_signature(manifest: dict[str, Any]) -> tuple[str, ...]:
    """Remove report-specific destinations while retaining profiler behavior and app arguments."""
    command = manifest.get('record_command')
    if not isinstance(command, list) or not all(isinstance(argument, str) for argument in command):
        raise ProfileError('profiling manifest has no valid record command')
    try:
        separator = command.index('--')
    except ValueError as error:
        raise ProfileError('profiling manifest record command has no application separator') from error
    profiler: list[str] = []
    index = 0
    while index < separator:
        argument = command[index]
        if argument in {'--output', '--profile-name'}:
            index += 2
            continue
        profiler.append(argument)
        index += 1
    application_arguments = command[separator + 2 :]
    return (*profiler, '--', *application_arguments)


def comparison_mismatches(analyses: list[ReportAnalysis]) -> list[str]:
    """Return environment differences which invalidate percentage comparisons."""
    baseline = analyses[0]
    checks = (
        ('mode', lambda manifest: manifest.get('mode')),
        ('host platform', lambda manifest: manifest.get('host', {}).get('platform')),
        ('host machine', lambda manifest: manifest.get('host', {}).get('machine')),
        ('perf access', lambda manifest: manifest.get('host', {}).get('perf_event_paranoid')),
        ('Rust compiler', lambda manifest: manifest.get('tools', {}).get('rustc')),
        ('Samply version', lambda manifest: manifest.get('tools', {}).get('samply')),
        ('build command', lambda manifest: manifest.get('build', {}).get('command')),
        ('build environment', lambda manifest: manifest.get('build', {}).get('environment')),
        ('record options', record_signature),
    )
    mismatches = []
    for analysis in analyses[1:]:
        for label, value in checks:
            if value(analysis.manifest) != value(baseline.manifest):
                mismatches.append(f'{analysis.directory.name}: {label} differs from {baseline.directory.name}')
    return mismatches


def print_comparison(analyses: list[ReportAnalysis], *, allow_incomparable: bool) -> None:
    mismatches = comparison_mismatches(analyses)
    if mismatches and not allow_incomparable:
        details = '\n  '.join(mismatches)
        raise ProfileError(
            f'profiling reports are not comparable:\n  {details}\n'
            'pass --allow-incomparable only when the mismatch is intentional'
        )
    labels = [analysis.directory.name for analysis in analyses]
    width = max(24, *(len(label) + 2 for label in labels))
    metrics = (
        ('interaction_frame', 'Interaction p95', ' ms'),
        ('interaction_desktop_ui', 'Interaction UI p95', ' ms'),
        ('interaction_map_ui', 'Interaction map p95', ' ms'),
        ('map_ui', 'Map UI p95', ' ms'),
        ('desktop_ui', 'Desktop UI p95', ' ms'),
        ('scene', 'Scene p95', ' ms'),
        ('route_query', 'Route query p95', ' µs'),
        ('render_draw', 'Render draw p95', ' ms'),
    )
    print('Performance report comparison\n')
    if mismatches:
        print('WARNING: incomparable reports were explicitly allowed')
        for mismatch in mismatches:
            print(f'  {mismatch}')
        print()
    print(f'{"Metric":<22}' + ''.join(f'{label:>{width}}' for label in labels))
    print('─' * (22 + width * len(labels)))
    print(
        f'{"Capture duration":<22}'
        + ''.join(f'{analysis.runtime.duration_seconds:.1f} s'.rjust(width) for analysis in analyses)
    )
    print(
        f'{"Interaction samples":<22}'
        + ''.join(
            f'{(analysis.runtime.interaction_frame.count if analysis.runtime.interaction_frame else 0):>{width}d}'
            for analysis in analyses
        )
    )
    print(
        f'{"Interaction seconds":<22}'
        + ''.join(f'{interaction_seconds(analysis.runtime):>{width}.1f}' for analysis in analyses)
    )
    for key, label, unit in metrics:
        print(f'{label:<22}', end='')
        previous = None
        for analysis in analyses:
            value = comparison_value(analysis.runtime, key)
            print(f'{comparison_cell(value, previous, unit):>{width}}', end='')
            previous = value
        print()
    print(f'{"Sampled CPU":<22}', end='')
    previous = None
    for analysis in analyses:
        value = analysis.cpu.sampled_cpu_seconds / analysis.runtime.duration_seconds
        print(f'{comparison_cell(value, previous, " cores"):>{width}}', end='')
        previous = value
    print()
    for analysis in analyses:
        if analysis.manifest.get('git', {}).get('dirty') is True:
            print(f'Warning: {analysis.directory.name} was captured from a dirty source tree.')


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest='command', required=True)
    analyze = commands.add_parser('analyze', help='summarize one profiling report')
    analyze.add_argument('report')
    compare = commands.add_parser('compare', help='compare two or more profiling reports')
    compare.add_argument('--allow-incomparable', action='store_true')
    compare.add_argument('reports', nargs='+')
    return result


def main() -> None:
    arguments = parser().parse_args()
    try:
        reports = [arguments.report] if arguments.command == 'analyze' else arguments.reports
        if arguments.command == 'compare' and len(reports) < 2:
            raise ProfileError('compare requires at least two profiling reports')
        analyses = [analyze_report(report_directory(report)) for report in reports]
        if arguments.command == 'analyze':
            print_analysis(analyses[0])
        else:
            print_comparison(analyses, allow_incomparable=arguments.allow_incomparable)
    except (OSError, json.JSONDecodeError, ProfileError) as error:
        print(f'Error: {error}', file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == '__main__':
    main()
