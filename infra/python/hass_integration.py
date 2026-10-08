"""Build and run the disposable HASS integration VM, retaining diagnostics."""

import os
import platform
import pty
import re
import select
import shlex
import signal
import subprocess
import sys
import tempfile
import termios
import time
from collections import deque
from collections.abc import Callable, Generator, Mapping, Sequence
from contextlib import contextmanager, nullcontext, redirect_stderr, redirect_stdout, suppress
from dataclasses import dataclass
from errno import EIO
from io import StringIO
from pathlib import Path
from types import FrameType
from typing import BinaryIO, Literal

import tyro
from rich.table import Table
from rich.text import Text

from cli_output import TerminalOutput, child_environment, strip_ansi, syntax

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = TerminalOutput()


@dataclass(frozen=True)
class Options:
    """Run the HASS browser/recovery suite. Requires Linux and read/write access to /dev/kvm."""

    driver: Path | None = None
    """Reuse a built Nix test-driver package for harness diagnostics."""

    suite: Literal['routes', 'reconnect'] = 'routes'
    """Suite to build when --driver is not supplied."""


class Interrupted(Exception):
    def __init__(self, signum: int) -> None:
        super().__init__(signal.Signals(signum).name)
        self.signum = signum


def interrupt(signum: int, _frame: FrameType | None) -> None:
    raise Interrupted(signum)


def stop(
    child: subprocess.Popen[bytes],
    grace: float = 10,
    *,
    reader: BinaryIO | None = None,
    log: BinaryIO | None = None,
    live: Callable[[bytes], None] | None = None,
) -> None:
    """Allow driver cleanup, then kill any remaining members of our process group."""
    with suppress(ProcessLookupError):
        os.killpg(child.pid, signal.SIGTERM)
    try:
        if reader is not None and log is not None:
            wait_logged(child, reader, log, grace, live=live)
        else:
            child.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        pass
    finally:
        with suppress(ProcessLookupError):
            os.killpg(child.pid, signal.SIGKILL)
        child.wait()
        if reader is not None and log is not None:
            with suppress(subprocess.TimeoutExpired):
                wait_logged(child, reader, log, 1, live=live)


@contextmanager
def terminal_log() -> Generator[tuple[BinaryIO, BinaryIO]]:
    """Capture terminal-only colors (including Nix) without giving children our input terminal."""
    master, slave = pty.openpty()
    with os.fdopen(master, 'rb', buffering=0) as reader, os.fdopen(slave, 'wb', buffering=0) as terminal:
        attributes = termios.tcgetattr(slave)
        attributes[1] &= ~termios.OPOST
        termios.tcsetattr(slave, termios.TCSANOW, attributes)
        yield reader, terminal


def wait_logged(
    child: subprocess.Popen[bytes],
    reader: BinaryIO,
    log: BinaryIO,
    timeout: float,
    *,
    live: Callable[[bytes], None] | None = None,
) -> int:
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise subprocess.TimeoutExpired(child.args, timeout)
        ready, _, _ = select.select([reader], [], [], min(0.05, remaining))
        if not ready:
            if child.poll() is not None:
                break
            continue
        try:
            data = os.read(reader.fileno(), 65536)
        except OSError as error:
            if error.errno != EIO:
                raise
            break
        if not data:
            break
        log.write(data)
        log.flush()
        if live is not None:
            live(data)
    return child.wait(timeout=max(0, deadline - time.monotonic()))


class LiveLines:
    def __init__(self, show: Callable[[str], bool] | None = None) -> None:
        self.pending = bytearray()
        self.show = show

    def print_line(self, data: bytes) -> None:
        line = data.decode('utf-8', errors='replace')
        if self.show is None or self.show(strip_ansi(line)):
            OUTPUT.console.print(Text.from_ansi(line), soft_wrap=True)

    def feed(self, data: bytes) -> None:
        self.pending.extend(data)
        while b'\n' in self.pending:
            line, _, remaining = self.pending.partition(b'\n')
            self.pending = bytearray(remaining)
            self.print_line(bytes(line))

    def finish(self) -> None:
        if self.pending:
            self.print_line(bytes(self.pending))
            self.pending.clear()


def browser_progress(line: str) -> bool:
    return bool(
        re.search(
            r': (?:(?:not )?ok \d+ - |# (?:Subtest:|tests |pass |fail |skipped |cancelled ))',
            line,
        )
        or line.startswith(('error:', '!!!', 'Traceback'))
    )


def run_command(
    command: Sequence[str],
    log: Path,
    *,
    timeout: float,
    environment: Mapping[str, str] | None = None,
    stdout: Path | None = None,
    live: Callable[[bytes], None] | None = None,
) -> None:
    with (
        log.open('wb') as errors,
        terminal_log() as (reader, terminal),
        stdout.open('wb') if stdout else nullcontext(terminal) as output,
    ):
        child = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=terminal,
            env=child_environment(environment),
            start_new_session=True,
        )
        try:
            terminal.close()
            code = wait_logged(child, reader, errors, timeout, live=live)
            if code:
                raise subprocess.CalledProcessError(code, command)
        finally:
            previous = {
                sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
            }
            try:
                stop(child, reader=reader, log=errors, live=live)
            finally:
                for sig, handler in previous.items():
                    signal.signal(sig, handler)


def stage(
    label: str,
    command: Sequence[str],
    log: Path,
    *,
    timeout: float,
    environment: Mapping[str, str] | None = None,
    stdout: Path | None = None,
    live_filter: Callable[[str], bool] | None = None,
) -> None:
    start = time.monotonic()
    OUTPUT.panel(syntax(shlex.join(command), 'bash'), title=label)
    OUTPUT.panel(Text(str(log)), title='Live output and retained log', style='dim')
    lines = LiveLines(live_filter)
    try:
        with OUTPUT.progress(label):
            run_command(command, log, timeout=timeout, environment=environment, stdout=stdout, live=lines.feed)
    finally:
        lines.finish()
    OUTPUT.panel(Text(f'Completed in {time.monotonic() - start:.1f}s.'), title=label, style='green')


def report_tests(results: Path) -> bool:
    journal = results / 'artifacts' / 'results' / 'journal.log'
    if not journal.exists():
        return False
    table = Table('Result', 'Browser test', box=None, padding=(0, 1))
    for line in strip_ansi(journal.read_text(errors='replace')).splitlines():
        if match := re.search(r': (not ok|ok) \d+ - (.+)$', line):
            passed = match[1] == 'ok'
            table.add_row(Text('PASS' if passed else 'FAIL', style='green' if passed else 'red'), Text(match[2]))
    if not table.row_count:
        return False
    OUTPUT.panel(table, title='Browser tests')
    return True


def failure(error: OSError | subprocess.SubprocessError) -> int:
    if isinstance(error, subprocess.CalledProcessError):
        code = error.returncode if error.returncode > 0 else 128 - error.returncode
        message = f'The subprocess exited with status {code}. See the retained logs for details.'
    elif isinstance(error, subprocess.TimeoutExpired):
        code, message = 124, f'The subprocess exceeded its {error.timeout:g}s timeout and was stopped.'
    elif isinstance(error, FileNotFoundError):
        code, message = 127, str(error)
    elif isinstance(error, PermissionError):
        code, message = 126, str(error)
    else:
        code, message = 1, str(error)
    OUTPUT.panel(Text(message), title=f'Integration failed · exit {code}', style='red', error=True)
    return code


def diagnostics(results: Path, log: Path) -> None:
    try:
        if not report_tests(results) and log.exists():
            with log.open(errors='replace') as contents:
                OUTPUT.ansi(''.join(deque(contents, maxlen=30)), title='Last subprocess output', error=True)
    except OSError as error:
        OUTPUT.panel(Text(str(error)), title='Could not read diagnostics', style='yellow', error=True)


def main(driver: Path | None = None, suite: Literal['routes', 'reconnect'] = 'routes') -> int:
    """Run the HASS browser/recovery suite. Requires Linux and read/write access to /dev/kvm.

    Args:
        driver: Reuse a built Nix test-driver package for harness diagnostics.
        suite: Select the route workflow suite or the socket/reconnect diagnostic matrix.
    """
    if platform.system() != 'Linux' or not os.access('/dev/kvm', os.R_OK | os.W_OK):
        OUTPUT.panel(
            Text('HASS integration tests require Linux and read/write access to /dev/kvm.'),
            title='KVM unavailable',
            style='red',
            error=True,
        )
        return 1
    parent = ROOT / '.tmp'
    parent.mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='hass-integration.', dir=parent))
    results = output / 'results'
    results.mkdir()
    OUTPUT.panel(Text(str(output)), title='Diagnostics directory')
    log = output / 'build.log'
    previous = {sig: signal.signal(sig, interrupt) for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        if driver is None:
            driver_path = output / 'driver-path'
            target = (
                f'checks.{platform.machine()}-linux.hass-integration.driver'
                if suite == 'routes'
                else 'hass-reconnect-driver'
            )
            stage(
                'Build integration driver',
                [
                    'nix',
                    'build',
                    '-L',
                    '--max-jobs',
                    '1',
                    '--cores',
                    '2',
                    '--no-update-lock-file',
                    '--no-link',
                    '--print-out-paths',
                    f'git+file://{ROOT}#{target}',
                ],
                log,
                timeout=3600,
                stdout=driver_path,
            )
            driver = Path(strip_ansi(driver_path.read_text()).strip())
        log = output / 'vm.log'
        # Virtiofs sockets need a short path to fit the Unix socket pathname limit.
        with tempfile.TemporaryDirectory(prefix='hass-vm.', dir='/tmp') as runtime:
            stage(
                'Run browser assertions and verify teardown',
                [str(driver / 'bin' / 'nixos-test-driver'), '-o', str(results)],
                log,
                timeout=1200,
                environment={**os.environ, 'XDG_RUNTIME_DIR': runtime, 'TMPDIR': runtime},
                live_filter=browser_progress,
            )
    except (Interrupted, KeyboardInterrupt) as error:
        OUTPUT.panel(
            Text('Owned processes stopped; diagnostics retained.'), title='Run cancelled', style='yellow', error=True
        )
        return 128 + (error.signum if isinstance(error, Interrupted) else signal.SIGINT)
    except (OSError, subprocess.SubprocessError) as error:
        code = failure(error)
        diagnostics(results, log)
        return code
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        OUTPUT.panel(Text(str(output)), title='Retained logs and artifacts')
    report_tests(results)
    OUTPUT.panel(Text('All assertions passed. VM stopped.'), title='HASS integration passed', style='green')
    return 0


def entrypoint(args: Sequence[str] | None = None) -> int:
    OUTPUT.install_tracebacks()
    captured_out, captured_error = StringIO(), StringIO()
    columns = os.environ.get('COLUMNS')
    code: int | None = None
    options: Options | None = None
    try:
        os.environ['COLUMNS'] = str(max(40, OUTPUT.console.width - 8))
        with redirect_stdout(captured_out), redirect_stderr(captured_error):
            options = tyro.cli(Options, args=args)
    except SystemExit as error:
        code = error.code if isinstance(error.code, int) else (0 if error.code is None else 1)
    finally:
        if columns is None:
            os.environ.pop('COLUMNS', None)
        else:
            os.environ['COLUMNS'] = columns
    if code is not None:
        OUTPUT.ansi(
            captured_out.getvalue() + captured_error.getvalue(),
            title='Usage error' if code else 'HASS integration',
            error=bool(code),
        )
        return code
    assert options is not None
    try:
        return main(options.driver, options.suite)
    except (Interrupted, KeyboardInterrupt) as error:
        OUTPUT.panel(Text('Run cancelled.'), title='Cancelled', style='yellow', error=True)
        return 128 + (error.signum if isinstance(error, Interrupted) else signal.SIGINT)
    except (OSError, subprocess.SubprocessError) as error:
        return failure(error)


if __name__ == '__main__':
    sys.exit(entrypoint())
