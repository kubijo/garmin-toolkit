"""Record and enrich a reproducible native desktop Samply profile."""

import argparse
import gzip
import hashlib
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Iterable, Sequence
from contextlib import suppress
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
REPORTS_ROOT = REPOSITORY_ROOT / '.tmp' / 'profiles'
PROFILE_NAME = 'profile.json.gz'
COMBINED_NAME = 'combined.json.gz'
METRICS_NAME = 'runtime-metrics.ndjson'
MANIFEST_NAME = 'manifest.json'
METRICS_ENVIRONMENT = 'GARMIN_TOOLKIT_RUNTIME_METRICS'
MANIFEST_SCHEMA_VERSION = 2


class ProfileError(RuntimeError):
    """A guard rejected a capture which would be absent or misleading."""


class ProfileCanceled(Exception):
    """The operator interrupted one named profiling phase."""

    def __init__(self, phase: str, report_dir: Path | None = None) -> None:
        self.phase = phase
        self.report_dir = report_dir
        super().__init__(phase)

    def __str__(self) -> str:
        if self.report_dir is None:
            return (
                f'Canceled during {self.phase}. Partial Cargo artifacts were retained; no profiling report was created.'
            )
        return f'Canceled during {self.phase}. Raw report state was preserved at {self.report_dir.resolve()}.'


@dataclass(frozen=True)
class BuildPlan:
    """The exact optimized build shared by pre-warming and recording."""

    mode: str
    command: tuple[str, ...]
    binary: Path
    environment: dict[str, str]
    rust_flags: str

    @classmethod
    def for_mode(cls, mode: str) -> BuildPlan:
        features = ('--features', 'demo') if mode == 'demo' else ()
        environment = os.environ.copy()
        inherited_flags = environment.get('RUSTFLAGS', '')
        rust_flags = f'{inherited_flags} -C force-frame-pointers=yes'.strip()
        environment['RUSTFLAGS'] = rust_flags
        return cls(
            mode=mode,
            command=(
                'cargo',
                'build',
                '--locked',
                '--profile',
                'profiling',
                '-p',
                'garmin-desktop',
                *features,
            ),
            binary=REPOSITORY_ROOT / '.tmp' / 'cargo-target' / 'profiling' / 'garmin-desktop',
            environment=environment,
            rust_flags=rust_flags,
        )


@dataclass(frozen=True)
class BuildResult:
    """Reproducibility metadata for one completed profiling build."""

    plan: BuildPlan
    started_at: str
    finished_at: str
    elapsed_seconds: float
    binary_sha256: str


@dataclass(frozen=True)
class ReportSlot:
    """A checked report name which can be reserved after the build succeeds."""

    path: Path
    recoverable_manifest_sha256: str | None = None

    @classmethod
    def inspect(cls, path: Path) -> ReportSlot:
        if path.is_symlink():
            raise ProfileError(f'{path} must not be a symbolic link')
        if not path.exists():
            return cls(path)
        if not path.is_dir():
            raise ProfileError(f'{path} exists and is not a report directory')

        entries = list(path.iterdir())
        manifest_path = path / MANIFEST_NAME
        if (
            len(entries) == 1
            and entries[0] == manifest_path
            and manifest_path.is_file()
            and not manifest_path.is_symlink()
        ):
            try:
                manifest = json.loads(manifest_path.read_text())
            except json.JSONDecodeError as error:
                raise ProfileError(f'{manifest_path} is not valid JSON') from error
            if isinstance(manifest, dict):
                legacy_pre_capture = manifest.get('schema_version') == 1 and manifest.get('status') == 'building'
                terminal_without_evidence = manifest.get('schema_version') == 2 and manifest.get('status') in {
                    'canceled',
                    'failed',
                }
                if legacy_pre_capture or terminal_without_evidence:
                    return cls(path, file_digest(manifest_path))

        raise ProfileError(f'{path} already contains profiling state; choose a new report name')

    def reserve(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        if self.recoverable_manifest_sha256 is not None:
            manifest_path = self.path / MANIFEST_NAME
            entries = list(self.path.iterdir()) if self.path.is_dir() else []
            if (
                len(entries) != 1
                or entries[0] != manifest_path
                or not manifest_path.is_file()
                or manifest_path.is_symlink()
                or file_digest(manifest_path) != self.recoverable_manifest_sha256
            ):
                raise ProfileError(f'{self.path} changed while the profiling binary was building')
            timestamp = datetime.now(UTC).strftime('%Y%m%dT%H%M%S%fZ')
            archived = self.path.parent / f'.{self.path.name}.abandoned-{timestamp}'
            self.path.rename(archived)
            print(f'Preserved prior manifest-only report at {archived.resolve()}')
        try:
            self.path.mkdir(exist_ok=False)
        except FileExistsError as error:
            raise ProfileError(f'{self.path} was claimed by another profiling run') from error


@dataclass
class ReportRun:
    """Own and validate the manifest lifecycle of one captured report."""

    directory: Path
    manifest: dict[str, Any]

    @classmethod
    def start(cls, slot: ReportSlot, manifest: dict[str, Any]) -> ReportRun:
        slot.reserve()
        run = cls(slot.path, manifest)
        try:
            run.write()
        except Exception:
            (slot.path / MANIFEST_NAME).with_suffix('.tmp').unlink(missing_ok=True)
            with suppress(OSError):
                slot.path.rmdir()
            raise
        return run

    def write(self) -> None:
        write_manifest(self.directory / MANIFEST_NAME, self.manifest)

    def transition(self, status: str, **fields: Any) -> None:
        current = self.manifest['status']
        allowed = {
            'capturing': {'finalizing', 'canceled', 'failed'},
            'finalizing': {'complete', 'canceled', 'failed'},
        }
        if status not in allowed.get(current, set()):
            raise ProfileError(f'invalid profiling report transition: {current} -> {status}')
        updated = {**self.manifest, 'status': status, 'phase': status, **fields}
        write_manifest(self.directory / MANIFEST_NAME, updated)
        self.manifest = updated

    def cancel(self, phase: str) -> None:
        self.transition('canceled', canceled_during=phase, finished_at=datetime.now(UTC).isoformat())

    def fail(self, error: Exception) -> None:
        self.transition('failed', failure=str(error), finished_at=datetime.now(UTC).isoformat())


@dataclass(frozen=True)
class CounterSpec:
    """One typed projection from runtime events into a Firefox Profiler counter."""

    name: str
    category: str
    field: str
    color: str
    gauge: bool
    event: str


COUNTERS = (
    CounterSpec(
        'Desktop frame interval', 'Desktop frame timing (ms)', 'frame_milliseconds', 'blue', False, 'desktop_frame'
    ),
    CounterSpec('Desktop UI', 'Desktop frame timing (ms)', 'ui_milliseconds', 'teal', False, 'desktop_frame'),
    CounterSpec('Map frame interval', 'Map frame timing (ms)', 'frame_milliseconds', 'blue', False, 'map_frame'),
    CounterSpec('Map UI', 'Map frame timing (ms)', 'ui_milliseconds', 'teal', False, 'map_frame'),
    CounterSpec('Scene acquisition', 'Map frame timing (ms)', 'scene_milliseconds', 'green', False, 'map_frame'),
    CounterSpec('Route query', 'Route query (µs)', 'route_query_microseconds', 'purple', False, 'map_frame'),
    CounterSpec('Label layout', 'Map frame timing (ms)', 'label_milliseconds', 'orange', False, 'map_frame'),
    CounterSpec('Label backlog', 'Map work', 'label_backlog', 'yellow', True, 'map_frame'),
    CounterSpec('Visible tiles', 'Map tiles', 'visible_tiles', 'blue', True, 'map_frame'),
    CounterSpec('Ready tiles', 'Map tiles', 'ready_tiles', 'green', True, 'map_frame'),
    CounterSpec('Pending tiles', 'Map tiles', 'pending_tiles', 'orange', True, 'map_frame'),
    CounterSpec('Queued upload bytes', 'Map upload', 'queued_upload_bytes', 'magenta', True, 'map_frame'),
    CounterSpec('Uploaded bytes', 'Map upload', 'uploaded_bytes', 'teal', False, 'map_frame'),
    CounterSpec('Stale work', 'Map work', 'stale_work', 'red', True, 'map_frame'),
    CounterSpec('Render prepare', 'Map render callback (ms)', 'milliseconds', 'purple', False, 'prepare'),
    CounterSpec('Render draw submission', 'Map render callback (ms)', 'milliseconds', 'orange', False, 'draw'),
)


def command_output(command: Sequence[str]) -> str:
    """Run a metadata command and return its trimmed standard output."""
    return subprocess.run(
        command,
        cwd=REPOSITORY_ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout.strip()


def require_tool(name: str) -> None:
    """Reject an environment missing a capture prerequisite."""
    if shutil.which(name) is None:
        raise ProfileError(f'{name} is unavailable; enter the repository development shell')


def require_perf_access(path: Path = Path('/proc/sys/kernel/perf_event_paranoid')) -> str | None:
    """Require unrestricted Linux perf events so missing samples cannot look valid."""
    if not sys.platform.startswith('linux') or not path.exists():
        return None

    value = path.read_text().strip()

    if value != '-1':
        raise ProfileError(
            f'kernel.perf_event_paranoid={value}; expected -1 for a complete Samply capture. '
            'Temporarily set it with: sudo sysctl kernel.perf_event_paranoid=-1'
        )

    return value


def run_checked(
    command: Sequence[str],
    *,
    phase: str,
    environment: dict[str, str] | None = None,
) -> None:
    """Run an inherited-terminal command and normalize operator interruption."""
    try:
        subprocess.run(command, cwd=REPOSITORY_ROOT, env=environment, check=True)
    except KeyboardInterrupt as error:
        raise ProfileCanceled(phase) from error
    except subprocess.CalledProcessError as error:
        if error.returncode in {-signal.SIGINT, 128 + signal.SIGINT}:
            raise ProfileCanceled(phase) from error
        raise


def build_profiling_binary(mode: str) -> BuildResult:
    """Build the accurate profiling binary while retaining Cargo's incremental work."""
    require_tool('cargo')
    plan = BuildPlan.for_mode(mode)
    print(
        'Preparing the optimized profiling binary. A cold build compiles the full dependency graph '
        'with frame pointers and may take several minutes.',
        flush=True,
    )
    print('Ctrl-C is safe: Cargo retains completed work and a rerun resumes the build.', flush=True)
    started_at = datetime.now(UTC).isoformat()
    started = time.monotonic()
    run_checked(plan.command, phase='profiling build', environment=plan.environment)
    elapsed_seconds = time.monotonic() - started
    finished_at = datetime.now(UTC).isoformat()

    if not plan.binary.is_file():
        raise ProfileError(f'profiling build did not produce {plan.binary}')

    result = BuildResult(
        plan=plan,
        started_at=started_at,
        finished_at=finished_at,
        elapsed_seconds=elapsed_seconds,
        binary_sha256=file_digest(plan.binary),
    )
    print(f'Profiling binary ready in {elapsed_seconds:.1f} seconds: {plan.binary}', flush=True)
    return result


def report_directory(name: str) -> Path:
    """Resolve one non-nested report name beneath the disposable profile root."""

    if not name or Path(name).name != name or name in {'.', '..'}:
        raise ProfileError('report must be one non-empty directory name')

    return REPORTS_ROOT / name


def write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    """Replace only the mutable run ledger, never a captured evidence file."""
    temporary = path.with_suffix('.tmp')
    temporary.write_text(f'{json.dumps(manifest, indent=2, sort_keys=True)}\n')
    temporary.replace(path)


def file_digest(path: Path) -> str:
    """Return the SHA-256 digest used to bind derived data to its inputs."""
    digest = hashlib.sha256()

    with path.open('rb') as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)

    return digest.hexdigest()


def run_manifest(
    mode: str,
    report: str,
    perf_access: str | None,
    build: BuildResult,
    record_command: Sequence[str],
) -> dict[str, Any]:
    """Capture enough context to reproduce and reject incomparable runs."""
    status = command_output(['git', 'status', '--porcelain=v1'])

    return {
        'schema_version': MANIFEST_SCHEMA_VERSION,
        'status': 'capturing',
        'phase': 'capturing',
        'capture_pid': os.getpid(),
        'started_at': datetime.now(UTC).isoformat(),
        'mode': mode,
        'report': report,
        'git': {
            'commit': command_output(['git', 'rev-parse', 'HEAD']),
            'dirty': bool(status),
            'status': status.splitlines(),
        },
        'host': {
            'platform': platform.platform(),
            'machine': platform.machine(),
            'perf_event_paranoid': perf_access,
        },
        'tools': {
            'rustc': command_output(['rustc', '--version']),
            'samply': command_output(['samply', '--version']),
        },
        'build': {
            'started_at': build.started_at,
            'finished_at': build.finished_at,
            'elapsed_seconds': build.elapsed_seconds,
            'command': list(build.plan.command),
            'environment': {'RUSTFLAGS': build.plan.rust_flags},
            'binary_sha256': build.binary_sha256,
        },
        'record_command': list(record_command),
    }


def copy_symbol_sidecars(report_dir: Path) -> list[Path]:
    """Give the derived profile its own copy of any raw presymbolication sidecar."""
    copied: list[Path] = []
    for source in sorted(report_dir.glob('profile*.syms.json')):
        destination = report_dir / source.name.replace('profile', 'combined', 1)
        shutil.copyfile(source, destination)
        copied.append(destination)
    return copied


def load_metrics(path: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    """Read and validate the immutable newline-delimited map metrics report."""
    rows: list[dict[str, Any]] = []
    for line_number, line in enumerate(path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        row = json.loads(line)
        if not isinstance(row, dict):
            raise ProfileError(f'{path}:{line_number}: expected a JSON object')
        rows.append(row)

    if not rows or rows[0].get('kind') != 'header':
        raise ProfileError(f'{path} has no metrics header')

    header = rows[0]
    if header.get('schema_version') != 1:
        raise ProfileError(f'{path} uses an unsupported metrics schema')

    samples = rows[1:]
    kinds = {'desktop_frame', 'map_frame', 'map_render'}

    if not samples or any(sample.get('kind') not in kinds for sample in samples):
        raise ProfileError(f'{path} contains an unsupported runtime sample')

    if not any(sample.get('kind') == 'map_frame' for sample in samples):
        raise ProfileError(f'{path} contains no complete map frames')

    elapsed = [numeric(sample, 'elapsed_milliseconds') for sample in samples]
    if elapsed != sorted(elapsed):
        raise ProfileError(f'{path} contains non-monotonic sample times')

    return header, samples


def numeric(row: dict[str, Any], key: str) -> float:
    """Read one finite profiler number without accepting booleans or schema drift."""
    value = row.get(key)

    if isinstance(value, bool) or not isinstance(value, int | float):
        raise ProfileError(f'metric {key} is not numeric')

    number = float(value)
    if not float('-inf') < number < float('inf'):
        raise ProfileError(f'metric {key} is not finite')

    return number


def profile_thread(profile: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    """Choose the sampled UI thread which owns the injected map counters."""
    threads = profile.get('threads')

    if not isinstance(threads, list) or not threads:
        raise ProfileError('Samply profile contains no threads')

    candidates = [
        (index, thread)
        for index, thread in enumerate(threads)
        if isinstance(thread, dict) and thread.get('name') in {'main', 'garmin-desktop'}
    ]

    if not candidates:
        candidates = [(index, thread) for index, thread in enumerate(threads) if isinstance(thread, dict)]

    return max(candidates, key=lambda item: int(item[1].get('samples', {}).get('length', 0)))


def differences(values: Iterable[float]) -> list[float]:
    """Convert an absolute gauge into counter deltas reconstructed by Firefox Profiler."""
    previous = 0.0
    result: list[float] = []

    for value in values:
        result.append(value - previous)
        previous = value

    return result


def make_counter(
    name: str,
    category: str,
    values: list[float],
    times: list[float],
    process_id: Any,
    thread_index: int,
    color: str,
) -> dict[str, Any]:
    """Build the counter shape emitted by Samply's processed-profile writer."""

    if len(values) != len(times):
        raise ProfileError(f'counter {name} has mismatched values and timestamps')

    return {
        'category': category,
        'name': name,
        'description': f'{name} per rendered map frame',
        'mainThreadIndex': thread_index,
        'pid': process_id,
        'samples': {
            'length': len(values),
            'count': values,
            'number': [1] * len(values),
            'time': times,
        },
        'color': color,
    }


def enrich_profile(profile: dict[str, Any], header: dict[str, Any], samples: list[dict[str, Any]]) -> int:
    """Add typed map counters to a loaded profile and return their count."""

    meta = profile.get('meta')

    if not isinstance(meta, dict):
        raise ProfileError('Samply profile has no metadata')

    profile_start = numeric(meta, 'startTime')
    metrics_start = numeric(header, 'started_unix_milliseconds')
    offset = metrics_start - profile_start
    thread_index, thread = profile_thread(profile)
    process_id = thread.get('pid')
    counters = profile.setdefault('counters', [])

    if not isinstance(counters, list):
        raise ProfileError('Samply profile counters have an unexpected shape')

    added = 0
    for spec in COUNTERS:
        present = [
            sample
            for sample in samples
            if sample.get(spec.field) is not None
            and (sample.get('kind') == spec.event or sample.get('phase') == spec.event)
        ]

        if not present:
            continue

        times = [offset + numeric(sample, 'elapsed_milliseconds') for sample in present]
        values = [numeric(sample, spec.field) for sample in present]

        if spec.gauge:
            values = differences(values)

        counters.append(make_counter(spec.name, spec.category, values, times, process_id, thread_index, spec.color))
        added += 1

    return added


def finalize(report_dir: Path) -> int:
    """
    Create a separate enriched profile while leaving both raw inputs untouched.
    """

    profile_path = report_dir / PROFILE_NAME
    metrics_path = report_dir / METRICS_NAME
    combined_path = report_dir / COMBINED_NAME

    if combined_path.exists():
        raise ProfileError(f'{combined_path} already exists; refusing to replace derived evidence')

    for path in (profile_path, metrics_path):
        if not path.is_file():
            raise ProfileError(f'required raw artifact is missing: {path}')

    original_profile_digest = file_digest(profile_path)
    original_metrics_digest = file_digest(metrics_path)

    with gzip.open(profile_path, 'rt') as source:
        profile = json.load(source)

    if not isinstance(profile, dict):
        raise ProfileError(f'{profile_path} has an unexpected root value')

    header, samples = load_metrics(metrics_path)
    counter_count = enrich_profile(profile, header, samples)

    with combined_path.open('xb') as raw_destination, gzip.open(raw_destination, 'wt') as destination:
        json.dump(profile, destination, separators=(',', ':'))

    if file_digest(profile_path) != original_profile_digest or file_digest(metrics_path) != original_metrics_digest:
        raise ProfileError('a raw profiling artifact changed while deriving the combined profile')

    copy_symbol_sidecars(report_dir)
    return counter_count


def capture(mode: str, report: str, samply_arguments: Sequence[str]) -> None:
    """Build, capture, enrich, and summarize one guarded profiling run."""
    for tool in ('git', 'rustc', 'samply'):
        require_tool(tool)
    perf_access = require_perf_access()
    report_dir = report_directory(report)
    slot = ReportSlot.inspect(report_dir)
    build = build_profiling_binary(mode)
    profile_path = report_dir / PROFILE_NAME
    metrics_path = report_dir / METRICS_NAME
    record_command = [
        'samply',
        'record',
        '--save-only',
        '--unstable-presymbolicate',
        '--cswitch-markers',
        '--output',
        str(profile_path),
        '--profile-name',
        f'Garmin Toolkit desktop ({mode}; {report})',
        '--reuse-threads',
        *samply_arguments,
        '--',
        str(build.plan.binary),
    ]
    manifest = run_manifest(mode, report, perf_access, build, record_command)
    run = ReportRun.start(slot, manifest)
    try:
        record_environment = os.environ.copy()
        record_environment[METRICS_ENVIRONMENT] = str(metrics_path)
        run_checked(record_command, phase='profile capture', environment=record_environment)
        run.transition('finalizing')
        counter_count = finalize(report_dir)
        artifacts = {
            path.name: {'bytes': path.stat().st_size, 'sha256': file_digest(path)}
            for path in sorted(report_dir.iterdir())
            if path.is_file() and path.name != MANIFEST_NAME
        }
        run.transition(
            'complete',
            finished_at=datetime.now(UTC).isoformat(),
            runtime_counter_count=counter_count,
            artifacts=artifacts,
        )
    except ProfileCanceled as error:
        run.cancel(error.phase)
        raise ProfileCanceled(error.phase, report_dir) from None
    except KeyboardInterrupt:
        phase = 'profile finalization' if run.manifest['status'] == 'finalizing' else 'profile capture'
        run.cancel(phase)
        raise ProfileCanceled(phase, report_dir) from None
    except Exception as error:
        run.fail(error)
        raise

    print(f'\nSaved guarded profiling report to {report_dir.resolve()}:')

    for path in sorted(report_dir.iterdir()):
        if path.is_file():
            print(f'  {path.name:36s} {path.stat().st_size / 1024:9.1f} KiB')

    print(f'\nView: samply load {(report_dir / COMBINED_NAME).resolve()}')
    print(f'Raw:  samply load {(report_dir / PROFILE_NAME).resolve()}')


def build_only(mode: str) -> None:
    """Pre-warm the exact build later reused by a profiling capture."""
    result = build_profiling_binary(mode)
    print(f'Binary SHA-256: {result.binary_sha256}')


def parser() -> argparse.ArgumentParser:
    """Build the command-line parser used by the just recipes and tests."""
    result = argparse.ArgumentParser(description=__doc__)

    commands = result.add_subparsers(dest='command', required=True)
    build = commands.add_parser('build', help='pre-warm the optimized profiling binary')
    build.add_argument('mode', choices=('production', 'demo'))
    record = commands.add_parser('record', help='build and record a guarded desktop profile')
    record.add_argument('mode', choices=('production', 'demo'))
    record.add_argument('report', nargs='?', default='00-latest')
    record.add_argument('samply_arguments', nargs=argparse.REMAINDER)
    finalize_command = commands.add_parser('finalize', help='derive a combined profile from raw artifacts')
    finalize_command.add_argument('report_dir', type=Path)

    return result


def main() -> None:
    """Run the selected profiling operation with concise guard failures."""
    arguments = parser().parse_args()

    try:
        if arguments.command == 'build':
            build_only(arguments.mode)
        elif arguments.command == 'record':
            samply_arguments = arguments.samply_arguments
            if samply_arguments[:1] == ['--']:
                samply_arguments = samply_arguments[1:]
            capture(arguments.mode, arguments.report, samply_arguments)
        else:
            count = finalize(arguments.report_dir.resolve())
            print(f'Added {count} map counters to {(arguments.report_dir / COMBINED_NAME).resolve()}')
    except ProfileCanceled as error:
        print(error, file=sys.stderr)
        raise SystemExit(130) from None
    except KeyboardInterrupt:
        print('Canceled before profiling began; no profiling report was created.', file=sys.stderr)
        raise SystemExit(130) from None
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError, ProfileError) as error:
        print(f'Error: {error}', file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == '__main__':
    main()
