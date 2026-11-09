import struct
import unittest

from connect_iq_protocol import Receiver, decode

# Independently specified little-endian frame: v1 snapshot, seq 0x1234,
# timestamp 0x01020304, battery 75%, charging, 1000 steps, HR 60, stress 25.
GOLDEN = bytes.fromhex('01013412040302014b000100e80300003c001900')


class ProtocolTests(unittest.TestCase):
    def test_golden_snapshot(self) -> None:
        snapshot = decode(GOLDEN, 0x1234)
        self.assertEqual(snapshot.timestamp, 0x01020304)
        self.assertEqual(snapshot.battery_percent, 75)
        self.assertTrue(snapshot.charging)
        self.assertEqual(snapshot.steps, 1000)
        self.assertEqual(snapshot.heart_rate_bpm, 60)
        self.assertEqual(snapshot.stress, 25)

    def test_missing_values_stay_missing(self) -> None:
        snapshot = decode(bytes.fromhex('0101000000000000ffffff00ffffffffffffffff'), 0)
        self.assertIsNone(snapshot.battery_percent)
        self.assertIsNone(snapshot.charging)
        self.assertIsNone(snapshot.steps)
        self.assertIsNone(snapshot.heart_rate_bpm)
        self.assertIsNone(snapshot.stress)

    def test_malformed_frames(self) -> None:
        for offset, value in ((0, 2), (1, 2), (2, 0), (8, 101), (10, 2), (11, 1), (18, 101)):
            payload = bytearray(GOLDEN)
            payload[offset] = value
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                decode(bytes(payload), 0x1234)
        for payload in (b'', GOLDEN[:-1], GOLDEN + b'\0'):
            with self.assertRaises(ValueError):
                decode(payload, 0x1234)

    def test_request_correlation_replay_and_disconnect(self) -> None:
        receiver = Receiver()
        self.assertEqual(receiver.request(), b'\x01\x01\0\0')
        with self.assertRaises(ValueError):
            receiver.request()
        with self.assertRaises(ValueError):
            receiver.receive(GOLDEN)
        response = GOLDEN[:2] + b'\0\0' + GOLDEN[4:]
        self.assertEqual(receiver.receive(response).sequence, 0)
        with self.assertRaises(ValueError):
            receiver.receive(response)
        self.assertEqual(receiver.request(), b'\x01\x01\x01\0')
        receiver.disconnect()
        with self.assertRaises(ValueError):
            receiver.receive(response)
        self.assertEqual(receiver.request(), b'\x01\x01\x02\0')

    def test_sequence_wrap(self) -> None:
        receiver = Receiver()
        for sequence in range(65537):
            self.assertEqual(receiver.request(), struct.pack('<BBH', 1, 1, sequence % 65536))
            response = GOLDEN[:2] + struct.pack('<H', sequence % 65536) + GOLDEN[4:]
            receiver.receive(response)
