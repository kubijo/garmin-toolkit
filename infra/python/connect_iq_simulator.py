"""Owner-local SDK simulator tests; no device or Bluetooth adapter access."""

import fcntl
import os
import platform
import socket
import subprocess
import time
import uuid
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

import connect_iq as local
from cli_output import child_environment
from connect_iq_manager import Runtime
from json_data import object_value, read_json, text


def simulator_ready() -> bool:
    try:
        with socket.create_connection(('127.0.0.1', 1234), timeout=0.2):
            return True
    except OSError:
        return False


@dataclass(frozen=True)
class SimulatorSession:
    sdk: Path
    key: Path
    output: Path
    environment: dict[str, str]
    process: subprocess.Popen[str]


def terminate(process: subprocess.Popen[str]) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


@contextmanager
def session(kind: Literal['test', 'preview']) -> Generator[SimulatorSession]:
    if platform.system() != 'Linux':
        raise ValueError('The owned simulator runtime currently supports Linux')
    state = local.ROOT / '.tmp/connect-iq'
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (state / 'simulator.lock').open('a') as lock:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError('Another owned simulator test or preview is running') from None
        if simulator_ready():
            raise ValueError('Simulator port 1234 is occupied; close the existing simulator before this owned run')
        settings = object_value(read_json(state / 'local.json', max_bytes=4096))
        sdk, key = Path(text(settings.get('sdk'))), Path(text(settings.get('key')))
        local.doctor(sdk=sdk, key=key)
        output = state / (kind + '-' + uuid.uuid4().hex)
        output.mkdir(mode=0o700)
        local.OUTPUT.panel(str(output), title='Retained simulator logs')
        runtime = Runtime(local.ROOT, state)
        runtime.ensure_image()
        runtime.verify(sdk / 'bin/simulator')
        environment = child_environment()
        environment['PATH'] = str(sdk / 'bin') + os.pathsep + environment['PATH']
        if simulator_ready():
            raise ValueError('Simulator port became occupied during runtime preparation')
        with (
            runtime.container(sdk / 'bin/simulator', gui=True, environment=environment) as command,
            (output / 'simulator.log').open('w') as stream,
        ):
            simulator = subprocess.Popen(
                command, stdin=subprocess.DEVNULL, stdout=stream, stderr=subprocess.STDOUT, text=True
            )
            try:
                deadline = time.monotonic() + 30
                while not simulator_ready():
                    if simulator.poll() is not None or time.monotonic() >= deadline:
                        raise ValueError('Simulator did not start; inspect ' + str(output / 'simulator.log'))
                    time.sleep(0.1)
                yield SimulatorSession(sdk, key, output, environment, simulator)
            finally:
                terminate(simulator)


def test(*, device: local.Device = 'edge1050') -> None:
    """Run SDK unit tests with coverage; the owned simulator closes after the tests."""
    with session('test') as simulator:
        result = subprocess.run(
            [
                'rafiki',
                '--color',
                'never',
                'coverage',
                'test',
                str(local.PROJECT / 'source'),
                '--jungle',
                str(local.PROJECT / 'monkey.jungle'),
                '--device',
                device,
                '--key',
                str(local.private_key(simulator.key)),
                '--instrument-out',
                str(simulator.output / 'coverage'),
                '--out-format',
                'lcov',
                '--out',
                str(simulator.output / 'coverage.info'),
            ],
            cwd=local.PROJECT,
            env=simulator.environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=180,
            check=False,
        )
        (simulator.output / 'tests.log').write_text(result.stdout)
        local.OUTPUT.ansi(result.stdout, title='Simulator test output')
        if result.returncode:
            raise subprocess.CalledProcessError(result.returncode, result.args)


def preview(*, device: local.Device = 'edge1050', duration: int = 3600) -> None:
    """Open the real probe UI until the simulator closes, Ctrl+C, or the duration limit (at most one hour)."""
    if not 1 <= duration <= 3600:
        raise ValueError('Preview duration must be from 1 to 3600 seconds')
    with session('preview') as simulator:
        output = simulator.output / 'build'
        local.build(sdk=simulator.sdk, key=simulator.key, output=output, device=device)
        command = [str(simulator.sdk / 'bin/monkeydo'), str(output / 'probe.prg'), device]
        with (simulator.output / 'app.log').open('w') as stream:
            client = subprocess.Popen(
                command,
                env=simulator.environment,
                stdin=subprocess.DEVNULL,
                stdout=stream,
                stderr=subprocess.STDOUT,
                text=True,
            )
            try:
                local.OUTPUT.panel(
                    'Close the simulator window or press Ctrl+C to finish.\nLimit: ' + str(duration) + ' seconds.',
                    title='Interactive probe preview',
                )
                deadline = time.monotonic() + duration
                while simulator.process.poll() is None and time.monotonic() < deadline:
                    if client.poll() not in (None, 0):
                        raise ValueError('Simulator app failed; inspect ' + str(simulator.output / 'app.log'))
                    time.sleep(0.2)
            finally:
                terminate(client)
