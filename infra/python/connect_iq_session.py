"""One foreground telemetry peer, with persistence before acknowledgement."""

from collections.abc import Callable

from connect_iq_protocol import Receiver, Snapshot, decode


class PeerBusy(ValueError):
    pass


class ProbeSession:
    def __init__(self, retain: Callable[[Snapshot], None]) -> None:
        self._retain = retain
        self._receiver = Receiver()
        self._peer: str | None = None
        self._pending: bytes | None = None
        self._last: bytes | None = None
        self.active = False

    def request(self, peer: str) -> bytes:
        if not self.active or not peer:
            raise PeerBusy('Telemetry service is not accepting requests')
        if self._peer is not None and self._peer != peer:
            raise PeerBusy('Another telemetry peer owns this connection')
        self._peer = peer
        if self._pending is None:
            self._pending = self._receiver.request()
        return self._pending

    def response(self, peer: str, payload: bytes) -> None:
        if not self.active or peer != self._peer:
            raise PeerBusy('No telemetry request belongs to this peer')
        if payload == self._last:
            return
        if self._pending is None:
            raise ValueError('No telemetry request is pending')
        snapshot = decode(payload, int.from_bytes(self._pending[2:], 'little'))
        self._retain(snapshot)
        self._receiver.receive(payload)
        self._pending = None
        self._last = payload

    def disconnect(self, peer: str) -> None:
        if self._peer == peer:
            self._receiver.disconnect()
            self._pending = None
            self._last = None
            self._peer = None

    def close(self) -> None:
        self.active = False
        if self._peer is not None:
            self.disconnect(self._peer)
