"""Explicit, bounded Linux BLE probe host. Importing this module never opens a bus."""

import asyncio
import json
import os
import platform
import re
import time
import uuid
from dataclasses import asdict
from pathlib import Path
from types import TracebackType

from dbus_fast.aio import MessageBus
from dbus_fast.constants import BusType

from cli_output import TerminalOutput
from connect_iq_bluez import BluezPeripheral, HostError
from connect_iq_protocol import Snapshot

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = TerminalOutput()


class Capture:
    def __init__(self, directory: Path) -> None:
        directory.mkdir(parents=True, exist_ok=False, mode=0o700)
        self.path = directory / 'telemetry.jsonl'
        descriptor = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        self._stream = os.fdopen(descriptor, 'w', encoding='utf-8')
        self.count = 0
        self.latest: Snapshot | None = None

    def retain(self, snapshot: Snapshot) -> None:
        line = (
            json.dumps({'schema_version': 1, 'received_at': time.time(), 'snapshot': asdict(snapshot)}, allow_nan=False)
            + '\n'
        )
        position = self._stream.tell()
        try:
            if self._stream.write(line) != len(line):
                raise OSError('Incomplete telemetry record')
            self._stream.flush()
        except OSError:
            self._stream.seek(position)
            self._stream.truncate()
            raise
        self.count += 1
        self.latest = snapshot

    def __enter__(self) -> Capture:
        return self

    def __exit__(
        self, _kind: type[BaseException] | None, _error: BaseException | None, _traceback: TracebackType | None
    ) -> None:
        self._stream.close()


async def serve(adapter: str, duration: int, capture: Capture) -> None:
    bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
    host = BluezPeripheral(bus, adapter, capture.retain, duration)
    waiters: list[asyncio.Task[None]] = []
    try:
        await host.start()
        OUTPUT.panel('TK Probe on ' + adapter + '\n' + str(capture.path), title='Telemetry host advertising')

        async def wait_for_stop() -> None:
            await host.stopped.wait()

        async def wait_for_disconnect() -> None:
            # dbus-fast shares this future with close(); cancelling an observer must not cancel it.
            await asyncio.shield(bus.wait_for_disconnect())

        stopped = asyncio.create_task(wait_for_stop())
        disconnected = asyncio.create_task(wait_for_disconnect())

        async def progress() -> None:
            reported = 0
            while True:
                if capture.count != reported:
                    reported = capture.count
                    OUTPUT.console.print('Received snapshots:', reported)
                await asyncio.sleep(1)

        monitor = asyncio.create_task(progress())
        waiters = [stopped, disconnected, monitor]
        done, _ = await asyncio.wait(
            [stopped, disconnected, monitor], timeout=duration, return_when=asyncio.FIRST_COMPLETED
        )
        if disconnected in done:
            raise HostError('D-Bus disconnected; telemetry host stopped')
        if monitor in done:
            await monitor
        if host.reason is not None:
            raise HostError(host.reason)
    finally:
        for waiter in waiters:
            waiter.cancel()
        await asyncio.gather(*waiters, return_exceptions=True)
        await host.close()
    OUTPUT.panel(str(capture.count) + ' snapshots retained at ' + str(capture.path), title='Telemetry host stopped')


def host(*, adapter: str, duration: int = 300, output: Path | None = None) -> None:
    """Advertise the probe service on an explicitly selected Linux adapter; requires physical-run authorization."""
    if platform.system() != 'Linux':
        raise ValueError('The BLE probe host uses Linux BlueZ')
    if not re.fullmatch(r'hci[0-9]+', adapter) or not 1 <= duration <= 3600:
        raise ValueError('Select an hciN adapter and a duration from 1 to 3600 seconds')
    directory = (output or ROOT / '.tmp/connect-iq' / ('capture-' + uuid.uuid4().hex)).resolve()
    if directory == ROOT / '.tmp' or not directory.is_relative_to(ROOT / '.tmp'):
        raise ValueError('Telemetry captures must use a new directory below repository .tmp')
    with Capture(directory) as capture:
        asyncio.run(serve(adapter, duration, capture))
