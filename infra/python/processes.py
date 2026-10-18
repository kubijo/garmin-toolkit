"""Stop owned process groups even when their original leader has already exited."""

import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
import uuid
from collections.abc import Callable, Generator
from contextlib import contextmanager, suppress
from pathlib import Path

ACCEPTANCE_SCOPE = 'GARMIN_ACCEPTANCE_SCOPE'
ACCEPTANCE_SCRATCH = 'GARMIN_ACCEPTANCE_SCRATCH'


@contextmanager
def cleanup_signals() -> Generator[None]:
    previous = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def vm_scope(owner: str) -> str:
    if re.fullmatch(r'garmin-acceptance-[0-9a-f]{32}\.scope', owner) is None:
        raise ValueError('Invalid acceptance scope owner')
    return owner.removesuffix('.scope') + '-vm.scope'


def session_scratch(base: Path) -> Path:
    """Use the controller's private scratch root while keeping evidence under the workspace."""
    owner, directory = os.environ.get(ACCEPTANCE_SCOPE), os.environ.get(ACCEPTANCE_SCRATCH)
    if owner is None and directory is None:
        return base
    if owner is None or directory is None:
        raise ValueError('Acceptance requires both its scope owner and scratch root')
    vm_scope(owner)
    path = Path(directory)
    if (
        path.is_symlink()
        or not path.is_dir()
        or path.resolve().parent != base.resolve()
        or not path.name.startswith('a.')
    ):
        raise ValueError('Acceptance scratch must be a private directory under repository .tmp')
    return path


def stop_scopes(owner: str) -> None:
    """Kill only this acceptance's two scopes, including descendants that called setsid."""
    units = (vm_scope(owner), owner)
    control = ['systemctl', '--user', '--no-ask-password']
    try:
        with suppress(subprocess.TimeoutExpired):
            subprocess.run(
                [*control, 'kill', '--kill-whom=all', '--signal=KILL', *units],
                check=False,
                capture_output=True,
                timeout=5,
            )
    finally:
        with suppress(subprocess.TimeoutExpired):
            subprocess.run([*control, 'stop', *units], check=False, capture_output=True, timeout=15)
    deadline = time.monotonic() + 5
    while True:
        result = subprocess.run(
            [*control, 'show', '--property=ActiveState', '--property=ControlGroup', *units],
            check=True,
            capture_output=True,
            text=True,
            timeout=5,
        )
        busy = False
        for row in result.stdout.splitlines():
            key, _, value = row.partition('=')
            if key == 'ActiveState' and value not in ('inactive', 'failed'):
                busy = True
            if key == 'ControlGroup' and value:
                events = Path('/sys/fs/cgroup') / value.lstrip('/') / 'cgroup.events'
                with suppress(FileNotFoundError):
                    busy |= 'populated 1' in events.read_text()
        if not busy:
            return
        if time.monotonic() >= deadline:
            raise RuntimeError('Owned acceptance scopes did not become empty')
        time.sleep(0.05)


def terminated(number: int, _frame: object) -> None:
    raise SystemExit(128 + number)


def run_owned(*arguments: str, timeout: float = 7200, grace: float = 45) -> None:
    """Reclaim owned processes before removing their private scratch; retain evidence separately."""
    owner = 'garmin-acceptance-' + uuid.uuid4().hex + '.scope'
    command = (
        'systemd-run',
        '--user',
        '--no-ask-password',
        '--scope',
        '--quiet',
        '--collect',
        '--expand-environment=no',
        '--unit=' + owner,
        '-p',
        'MemoryMax=6G',
        '-p',
        'CPUQuota=200%',
        '-p',
        'TimeoutStopSec=5s',
        '-p',
        f'RuntimeMaxSec={timeout + grace + 15}',
        *arguments,
    )
    previous = signal.signal(signal.SIGTERM, terminated)
    scratch: Path | None = None
    try:
        base = Path.cwd() / '.tmp'
        base.mkdir(parents=True, exist_ok=True)
        scratch = Path(tempfile.mkdtemp(prefix='a.', dir=base))
        process = subprocess.Popen(
            command,
            start_new_session=True,
            env={**os.environ, ACCEPTANCE_SCOPE: owner, ACCEPTANCE_SCRATCH: str(scratch), 'TMPDIR': str(scratch)},
        )
        try:
            status = process.wait(timeout=timeout)
            if status:
                raise subprocess.CalledProcessError(status, arguments)
        finally:
            stop_group(process, grace=grace)
    finally:
        try:
            with cleanup_signals():
                if scratch is not None:
                    try:
                        stop_scopes(owner)
                        shutil.rmtree(scratch)
                    except BaseException as error:
                        error.add_note(f'Acceptance scratch retained at {scratch}; scope owner {owner}')
                        raise
        finally:
            signal.signal(signal.SIGTERM, previous)


def stop_group[T: (str, bytes)](
    process: subprocess.Popen[T], grace: float = 10, *, wait: Callable[[float], object] | None = None
) -> None:
    with cleanup_signals():
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            if wait is None:
                process.wait(timeout=grace)
            else:
                wait(grace)
        except subprocess.TimeoutExpired:
            pass
        finally:
            with suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
