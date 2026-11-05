"""Bounded QMP requests for the desktop acceptance VM."""

import json
import socket
import time
from contextlib import ExitStack
from pathlib import Path

from json_data import Object, object_value


class Qmp:
    def __init__(self, path: Path) -> None:
        with ExitStack() as cleanup:
            self.socket = socket.socket(socket.AF_UNIX)
            cleanup.callback(self.socket.close)
            self.socket.settimeout(10)
            self.socket.connect(str(path))
            self.stream = self.socket.makefile('rwb')
            cleanup.callback(self.stream.close)
            greeting = self.read()
            if 'QMP' not in greeting:
                raise ValueError('Missing QMP greeting')
            self.command('qmp_capabilities')
            cleanup.pop_all()

    def close(self) -> None:
        self.stream.close()
        self.socket.close()

    def read(self) -> Object:
        line = self.stream.readline(1024 * 1024)
        if not line or not line.endswith(b'\n'):
            raise ValueError('Missing or oversized QMP response')
        return object_value(json.loads(line))

    def command(self, command: str, arguments: Object | None = None) -> object:
        deadline = time.monotonic() + 10
        self.stream.write(json.dumps({'execute': command, 'arguments': arguments or {}}).encode() + b'\n')
        self.stream.flush()
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f'QMP {command} timed out')
            self.socket.settimeout(remaining)
            reply = self.read()
            if 'error' in reply:
                raise ValueError(f'QMP {command}: {reply["error"]}')
            if 'return' in reply:
                return reply['return']

    def attach(self, vendor: int, product: int) -> None:
        devices = self.command('human-monitor-command', {'command-line': 'info usbhost'})
        if not isinstance(devices, str):
            raise ValueError('QEMU did not return its host USB inventory')
        identity = f'{vendor:04x}:{product:04x}'
        if devices.lower().count(identity) != 1:
            raise ValueError(f'Expected exactly one host USB device {identity}; found:\n{devices}')
        self.command('device_add', {'driver': 'usb-host', 'id': 'garmin', 'vendorid': vendor, 'productid': product})

    def screenshot(self, destination: Path) -> None:
        self.command('screendump', {'filename': str(destination), 'format': 'png'})
