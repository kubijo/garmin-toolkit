import json
import tempfile
import unittest
from pathlib import Path

from connect_iq_host import Capture
from connect_iq_protocol import Snapshot
from connect_iq_session import PeerBusy, ProbeSession
from test_connect_iq_protocol import GOLDEN


def frame(sequence: int = 0) -> bytes:
    return GOLDEN[:2] + sequence.to_bytes(2, 'little') + GOLDEN[4:]


class SessionTests(unittest.TestCase):
    def test_peer_lease_request_correlation_and_idempotent_acknowledgement(self) -> None:
        retained: list[Snapshot] = []
        session = ProbeSession(retained.append)
        with self.assertRaises(PeerBusy):
            session.request('/test/first')
        session.active = True
        first = session.request('/test/first')
        self.assertEqual(first, b'\x01\x01\0\0')
        self.assertEqual(session.request('/test/first'), first)
        with self.assertRaises(PeerBusy):
            session.request('/test/second')
        with self.assertRaises(PeerBusy):
            session.response('/test/second', frame())
        with self.assertRaises(ValueError):
            session.response('/test/first', frame(1))
        session.response('/test/first', frame())
        session.response('/test/first', frame())
        self.assertEqual(len(retained), 1)
        self.assertEqual(session.request('/test/first'), b'\x01\x01\x01\0')
        session.response('/test/first', frame())  # ACK retry must not consume the new request.
        session.response('/test/first', frame(1))
        self.assertEqual(len(retained), 2)

    def test_failed_retention_leaves_the_request_retryable(self) -> None:
        retained: list[Snapshot] = []
        fail = True

        def retain(snapshot: Snapshot) -> None:
            if fail:
                raise OSError('synthetic storage failure')
            retained.append(snapshot)

        session = ProbeSession(retain)
        session.active = True
        request = session.request('/test/first')
        with self.assertRaises(OSError):
            session.response('/test/first', frame())
        self.assertEqual(session.request('/test/first'), request)
        fail = False
        session.response('/test/first', frame())
        self.assertEqual(len(retained), 1)

    def test_disconnect_releases_only_the_matching_peer_and_invalidates_old_frames(self) -> None:
        session = ProbeSession(lambda _snapshot: None)
        session.active = True
        session.request('/test/first')
        session.disconnect('/test/other')
        with self.assertRaises(PeerBusy):
            session.request('/test/second')
        session.disconnect('/test/first')
        self.assertEqual(session.request('/test/second'), b'\x01\x01\x01\0')
        with self.assertRaises(ValueError):
            session.response('/test/second', frame())
        session.close()
        with self.assertRaises(PeerBusy):
            session.response('/test/second', frame(1))

    def test_capture_is_private_and_never_replaces_an_existing_trace(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / 'capture'
            snapshot = Snapshot(0, 123, 75, True, 1000, None, None)
            with Capture(directory) as capture:
                capture.retain(snapshot)
                self.assertEqual(capture.count, 1)
                self.assertEqual(capture.path.stat().st_mode & 0o777, 0o600)
            records = [json.loads(line) for line in capture.path.read_text().splitlines()]
            self.assertEqual(len(records), 1)
            self.assertEqual(records[0]['snapshot']['steps'], 1000)
            self.assertIsNone(records[0]['snapshot']['heart_rate_bpm'])
            with self.assertRaises(FileExistsError):
                Capture(directory)
