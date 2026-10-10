"""Trace development builds without starting an application or changing source files."""

import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Callable
from contextlib import suppress
from dataclasses import dataclass, field
from pathlib import Path
from types import FrameType
from typing import Any, Literal

import tyro
from jinja2 import Environment, FileSystemLoader, StrictUndefined, select_autoescape

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / '.tmp' / 'build-profile'

type Target = Literal['web', 'desktop', 'hass', 'gallery', 'cli', 'ui']
type Mode = Literal['demo', 'production']


@dataclass(frozen=True)
class Options:
    """Capture a development build without starting an application."""

    target: Target
    """Application to build."""
    label: str
    """Unique report directory under .tmp/build-profile."""
    mode: Mode = 'demo'
    profile: str = 'dev'
    incremental: Literal['0', '1'] = '1'
    target_dir: Path | None = None
    config: list[str] = field(default_factory=list)
    """Cargo configuration overrides for a controlled experiment."""
    compiler_profile: bool = False
    """Profile compiler internals for the UI or desktop library using the diagnostic nightly shell."""


def event(name: str, phase: str, **args: Any) -> None:
    record = dict(name=name, ph=phase, ts=time.monotonic_ns() // 1000, pid=1, tid=os.getpid(), args=args)
    with (Path(os.environ['GARMIN_BUILD_TRACE']) / f'{os.getpid()}.jsonl').open('a') as output:
        output.write(json.dumps(record) + '\n')


def invoke(command: list[str], name: str, *, group: bool = False, **options: Any) -> int:
    event(name, 'B', command=command, cwd=str(options.get('cwd', Path.cwd())))
    previous: dict[signal.Signals, Callable[[int, FrameType | None], Any] | int | None] = {}
    code = 1
    try:
        # Preserve the descriptors the caller marked inheritable, including Cargo's jobserver pipes.
        # Python's own files are close-on-exec by default.
        child = subprocess.Popen(command, close_fds=False, start_new_session=group, **options)

        def forward(signum: int, _frame: Any) -> None:
            with suppress(ProcessLookupError):
                if group:
                    os.killpg(child.pid, signum)
                else:
                    child.send_signal(signum)

        for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            previous[signum] = signal.signal(signum, forward)
        code = child.wait()
        code = code if code >= 0 else 128 - code
        return code
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        event(name, 'E', code=code)


def wrapper() -> int:
    tool = Path(sys.argv[1]).name
    args = sys.argv[2:]
    name = tool
    if tool == 'rustc-wrapper':
        command = args.pop(0)
        name = 'rustc ' + (args[args.index('--crate-name') + 1] if '--crate-name' in args else 'probe')
    else:
        command = json.loads(os.environ['GARMIN_BUILD_TOOLS'])[tool]
    if tool == 'cargo' and args and args[0] in ('build', 'rustc'):
        options = ['--timings']
        for config in json.loads(os.environ.get('GARMIN_BUILD_CONFIG', '[]')):
            options.extend(['--config', config])
        # cargo rustc passes everything after -- to rustc, not Cargo.
        args[1:1] = options
    return invoke([command, *args], name)


def spans(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    pending: dict[int, list[dict[str, Any]]] = {}
    result: list[dict[str, Any]] = []
    for item in sorted(events, key=lambda item: item['ts']):
        stack = pending.setdefault(item['tid'], [])
        if item['ph'] == 'B':
            stack.append(item)
        elif item['ph'] == 'E' and stack:
            begin = stack.pop()
            result.append({**begin, 'dur': item['ts'] - begin['ts'], 'args': {**begin['args'], **item['args']}})
    for stack in pending.values():
        for begin in stack:
            result.append({**begin, 'dur': 0, 'args': {**begin['args'], 'incomplete': True}})
    return result


def report_html(measured: list[dict[str, Any]], metadata: dict[str, Any]) -> str:
    duration = max(1, max((item['ts'] + item['dur'] for item in measured), default=1))
    environment = Environment(
        loader=FileSystemLoader(Path(__file__).parent / 'templates'),
        autoescape=select_autoescape(['html']),
        undefined=StrictUndefined,
    )
    return environment.get_template('build-profile.html').render(
        duration=duration,
        measured=sorted(measured, key=lambda item: item['ts']),
        metadata=metadata,
    )


def setup_tools() -> tuple[Path, dict[str, str]]:
    # Stable paths avoid profiler-induced Cargo cache invalidation between reports.
    bin_dir = BASE / 'bin'
    bin_dir.mkdir(parents=True, exist_ok=True)
    tools: dict[str, str] = {}
    for tool in ('cargo', 'wasm-bindgen', 'esbuild', 'node'):
        path = shutil.which(tool)
        if path:
            tools[tool] = path
    if 'cargo' not in tools:
        raise ValueError('cargo is missing; use the pinned development shell')
    for tool in (*tools, 'rustc-wrapper'):
        launcher = bin_dir / tool
        entrypoint = Path(__file__).with_name('build_tool.py')
        if launcher.is_symlink() and launcher.resolve() == entrypoint:
            continue
        launcher.unlink(missing_ok=True)
        launcher.symlink_to(entrypoint)
    return bin_dir, tools


def build_command(
    target: Target, mode: Mode, profile: str, compiler_output: Path | None = None
) -> tuple[list[str], Path]:
    if compiler_output is not None and target not in ('ui', 'desktop'):
        raise ValueError('compiler profiling supports the ui and desktop libraries')
    if target == 'web':
        return [
            'trunk',
            'build',
            '--locked',
            '--cargo-profile',
            profile,
            '--dist',
            str(BASE / 'dist'),
            'index.html',
        ], ROOT / 'apps' / 'garmin-hass' / 'web'
    command = ['cargo', 'build', '--locked', '--profile', profile]
    if target == 'gallery':
        return [*command, '--all-features'], ROOT / 'infra' / 'gallery'
    command.extend(['-p', f'garmin-{target}'])
    if mode == 'demo' and target == 'ui':
        command.extend(['--features', 'automation'])
    if mode == 'demo' and target in ('desktop', 'hass'):
        command.extend(['--features', 'demo'])
    if compiler_output is not None:
        command[1] = 'rustc'
        command.extend(['--lib', '--', f'-Zself-profile={compiler_output}', '-Zself-profile-events=default,args'])
    return command, ROOT


def compiler_reports(directory: Path) -> None:
    profiles = sorted(directory.glob('*.mm_profdata'))
    if not profiles:
        raise ValueError('compiler succeeded but produced no self-profile data')
    for profile in profiles:
        prefix = profile.with_suffix('')
        with prefix.with_suffix('.txt').open('w') as output:
            subprocess.run(['summarize', 'summarize', str(profile)], stdout=output, check=True)
        subprocess.run(['summarize', 'summarize', '--json', str(profile)], check=True)
    subprocess.run(['crox', '--dir', str(directory), '--minimum-duration', '50'], cwd=directory, check=True)


def collect_compiler_profiles(source: Path, destination: Path, previous: set[Path]) -> bool:
    fresh = set(source.glob('*.mm_profdata')) - previous
    for profile in sorted(fresh):
        shutil.move(profile, destination / profile.name)
    if fresh:
        compiler_reports(destination)
    return bool(fresh)


def capture(args: Options) -> int:
    if not re.fullmatch(r'[a-zA-Z0-9_-]+', args.label):
        raise ValueError('label must be a simple unique name')
    if os.environ.get('RUSTC_WRAPPER') or os.environ.get('RUSTC_WORKSPACE_WRAPPER'):
        raise ValueError('unset existing compiler wrappers before profiling')
    compiler_version = subprocess.check_output(['rustc', '--version', '--verbose'], text=True)
    if args.compiler_profile:
        if args.target not in ('ui', 'desktop'):
            raise ValueError('compiler profiling supports the ui and desktop libraries')
        if 'nightly' not in compiler_version or not all(shutil.which(tool) for tool in ('summarize', 'crox')):
            raise ValueError('use just dev::compiler-profile for the pinned nightly and measureme tools')
        if args.target_dir is not None:
            raise ValueError('compiler profiles use their isolated nightly target directory')
    BASE.mkdir(parents=True, exist_ok=True)
    output = BASE / args.label
    output.mkdir()  # Preserve previous measurements.
    events_dir = output / 'events'
    events_dir.mkdir()
    bin_dir, tools = setup_tools()
    target_dir = args.target_dir or (
        BASE / 'nightly-target'
        if 'nightly' in compiler_version
        else BASE / 'target'
        if args.target == 'web'
        else ROOT / '.tmp' / 'gallery-check-target'
        if args.target == 'gallery'
        else ROOT / '.tmp' / 'cargo-target'
    )
    os.environ['GARMIN_BUILD_TRACE'] = str(events_dir)
    environment = {
        **os.environ,
        'PATH': f'{bin_dir}:{os.environ["PATH"]}',
        'GARMIN_BUILD_TOOLS': json.dumps(tools),
        'GARMIN_BUILD_CONFIG': json.dumps(args.config),
        'CARGO_TARGET_DIR': str(target_dir.resolve()),
        'CARGO_INCREMENTAL': args.incremental,
        'CARGO_BUILD_JOBS': os.environ.get('CARGO_BUILD_JOBS', '2'),
        'RUSTC_WRAPPER': str(bin_dir / 'rustc-wrapper'),
        'TRUNK_SKIP_VERSION_CHECK': 'true',
        'TRUNK_COLOR': 'never',
    }
    environment.pop('NO_COLOR', None)  # Trunk rejects the conventional NO_COLOR=1.
    compiler_output = output / 'compiler' if args.compiler_profile else None
    compiler_raw = BASE / 'compiler-raw' / args.target if args.compiler_profile else None
    previous_profiles: set[Path] = set()
    if compiler_output is not None:
        compiler_output.mkdir()
    if compiler_raw is not None:
        compiler_raw.mkdir(parents=True, exist_ok=True)
        previous_profiles = set(compiler_raw.glob('*.mm_profdata'))
    # Keep rustc arguments stable: changing only the report path otherwise changes
    # Cargo's extra-filename and unnecessarily invalidates LLVM artifacts.
    command, cwd = build_command(args.target, args.mode, args.profile, compiler_raw)
    metadata = {
        'target': args.target,
        'mode': args.mode,
        'profile': args.profile,
        'incremental': args.incremental,
        'jobs': environment['CARGO_BUILD_JOBS'],
        'config': args.config,
        'compiler_version': compiler_version,
        'compiler_profile': args.compiler_profile,
        'compiler_profile_recorded': False,
        'compiler_trace_minimum_duration_us': 50 if args.compiler_profile else None,
        'target_rustflags': {
            name: value
            for name, value in environment.items()
            if name.startswith('CARGO_TARGET_') and name.endswith('_RUSTFLAGS')
        },
        'target_dir': str(target_dir),
        'tools': {**tools, 'trunk': shutil.which('trunk')},
        'bindgen_cache_disabled': 'TRUNK_BINDGEN_CACHE_DISABLE' in environment,
        'source_hashes': {
            path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
            for path in (
                'Cargo.toml',
                'Cargo.lock',
                'apps/garmin-hass/web/index.html',
                'apps/garmin-hass/web/Trunk.toml',
                'crates/garmin-ui/src/activity/workspace.rs',
                'crates/garmin-ui/src/profile.rs',
            )
        },
        'scope': 'Process wall times; excludes Nix shell startup, packaging and application reload. '
        'rustc spans include linking. Wrapper overhead is included.',
    }
    started = time.time()
    code = 1
    try:
        code = invoke(command, f'{args.target} build', group=True, cwd=cwd, env=environment)
        if code == 0 and compiler_output is not None and compiler_raw is not None:
            code = 1
            metadata['compiler_profile_recorded'] = collect_compiler_profiles(
                compiler_raw, compiler_output, previous_profiles
            )
            if not metadata['compiler_profile_recorded']:
                print('Cargo reused the library; edit source before capturing a new compiler profile.')
            code = 0
        return code
    finally:
        metadata['exit_code'] = code
        events = [json.loads(line) for file in events_dir.glob('*.jsonl') for line in file.read_text().splitlines()]
        start = min((item['ts'] for item in events), default=0)
        for item in events:
            item['ts'] -= start
        measured = spans(events)
        (output / 'trace.json').write_text(json.dumps({'traceEvents': events}))
        (output / 'spans.json').write_text(json.dumps(measured, indent=2))
        (output / 'metadata.json').write_text(json.dumps(metadata, indent=2))
        (output / 'index.html').write_text(report_html(measured, metadata))
        timings = target_dir / 'cargo-timings' / 'cargo-timing.html'
        if timings.exists() and timings.stat().st_mtime >= started:
            shutil.copyfile(timings, output / 'cargo-timing.html')
        print(f'\nBuild timeline: {output / "index.html"}')
        for item in sorted(measured, key=lambda item: -item['dur'])[:12]:
            print(f'{item["dur"] / 1e6:9.3f} s  {item["name"]}')


def main() -> int:
    args = tyro.cli(Options, config=(tyro.conf.PositionalRequiredArgs,))
    if args.compiler_profile:
        BASE.mkdir(parents=True, exist_ok=True)
        # The stable raw-output directory is shared; serialize collection as well
        # as compilation so another invocation cannot claim this run's profiles.
        with (BASE / 'compiler-profile.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            return capture(args)
    return capture(args)


if __name__ == '__main__':
    sys.exit(main())
