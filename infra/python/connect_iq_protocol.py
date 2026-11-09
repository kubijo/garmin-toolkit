"""Offline contract for a foreground Connect IQ telemetry probe; no radio access."""

import struct
from dataclasses import dataclass
from typing import cast

SERVICE_UUID = '59102400-299d-4b88-a1ce-668c821d87ca'
REQUEST_UUID = '59102401-299d-4b88-a1ce-668c821d87ca'
RESPONSE_UUID = '59102402-299d-4b88-a1ce-668c821d87ca'
REQUEST = struct.Struct('<BBH')
RESPONSE = struct.Struct('<BBHIHBBIHH')


@dataclass(frozen=True)
class Snapshot:
    sequence: int
    timestamp: int
    battery_percent: int | None
    charging: bool | None
    steps: int | None
    heart_rate_bpm: int | None
    stress: int | None


def measurement(value: int, missing: int, maximum: int) -> int | None:
    if value == missing:
        return None
    if not 0 <= value <= maximum:
        raise ValueError('Telemetry measurement outside its range')
    return value


def decode(payload: bytes, sequence: int) -> Snapshot:
    if len(payload) != RESPONSE.size:
        raise ValueError('Telemetry response must be exactly 20 bytes')
    version, kind, received, timestamp, battery, charging, reserved, steps, heart_rate, stress = cast(
        tuple[int, ...], RESPONSE.unpack(payload)
    )
    if (version, kind, received, reserved) != (1, 1, sequence, 0):
        raise ValueError('Unexpected version, response kind, sequence, or reserved field')
    if charging not in (0, 1, 255):
        raise ValueError('Invalid charging flag')
    return Snapshot(
        received,
        timestamp,
        measurement(battery, 65535, 100),
        None if charging == 255 else bool(charging),
        measurement(steps, 4294967295, 4294967294),
        measurement(heart_rate, 65535, 65534),
        measurement(stress, 65535, 100),
    )


class Receiver:
    """One outstanding snapshot request per connection; unsolicited/replayed writes fail."""

    def __init__(self) -> None:
        self._next = 0
        self._pending: int | None = None

    def request(self) -> bytes:
        if self._pending is not None:
            raise ValueError('A telemetry request is already pending')
        self._pending = self._next
        self._next = (self._next + 1) % 65536
        return REQUEST.pack(1, 1, self._pending)

    def receive(self, payload: bytes) -> Snapshot:
        if self._pending is None:
            raise ValueError('No telemetry request is pending')
        snapshot = decode(payload, self._pending)
        self._pending = None
        return snapshot

    def disconnect(self) -> None:
        self._pending = None
