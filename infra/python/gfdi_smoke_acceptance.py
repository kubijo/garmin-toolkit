"""Run the compiled Linux smoke driver against a private, synthetic BlueZ bus."""

import asyncio
import json
import os
import signal
import unittest
import uuid
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import ClassVar, cast

import tyro
from dbus_fast import Message, Variant
from dbus_fast.aio import MessageBus
from dbus_fast.constants import MessageType

ROOT = Path(__file__).resolve().parents[2]
PROPERTIES = 'org.freedesktop.DBus.Properties'
ADAPTER = '/org/bluez/hci0'
DEVICE = ADAPTER + '/dev_AA_BB_CC_DD_EE_FF'
SERVICE = DEVICE + '/service0001'
RECEIVE = SERVICE + '/char0002'
SEND = SERVICE + '/char0003'
CHARACTERISTIC = 'org.bluez.GattCharacteristic1'
# Synthetic identity fixture: no captured hardware identifiers or personal data.
IDENTITY = bytes.fromhex(
    '00023a04a0139635341204030201d20477010b44656d6f204761726d696e'
    '0e53796e74686574696320706565720c44656d6f20747261636b6572a63d00'
)


def frame(kind: int, payload: bytes) -> bytes:
    """Independent fixture encoder: little-endian envelope, CRC-16/ARC, Garmin COBS."""
    raw = (len(payload) + 6).to_bytes(2, 'little') + kind.to_bytes(2, 'little') + payload
    checksum = 0
    for byte in raw:
        checksum ^= byte
        for _ in range(8):
            checksum = (checksum >> 1) ^ (0xA001 if checksum & 1 else 0)
    raw += checksum.to_bytes(2, 'little')
    encoded = bytearray([0])
    for block in raw.split(b'\0'):
        if len(block) >= 254:
            raise AssertionError('Fixture exceeds short COBS block')
        encoded.append(len(block) + 1)
        encoded.extend(block)
    return bytes(encoded) + b'\0'


def unframe(encoded: bytes) -> tuple[int, bytes]:
    raw = bytearray()
    offset = 0
    while offset < len(encoded):
        length = encoded[offset]
        if length == 0 or offset + length > len(encoded):
            raise AssertionError('Malformed COBS fixture')
        raw.extend(encoded[offset + 1 : offset + length])
        offset += length
        if length != 255 and offset < len(encoded):
            raw.append(0)
    if len(raw) < 6 or int.from_bytes(raw[:2], 'little') != len(raw):
        raise AssertionError('Invalid fixture envelope')
    kind = int.from_bytes(raw[2:4], 'little')
    if frame(kind, bytes(raw[4:-2])) != b'\0' + encoded + b'\0':
        raise AssertionError('Invalid fixture CRC')
    return kind, bytes(raw[4:-2])


class FakeBluez:
    def __init__(self, bus: MessageBus, scenario: str) -> None:
        self.bus = bus
        self.scenario = scenario
        self.calls: list[str] = []
        self.writes: list[bytes] = []
        self.tasks: list[asyncio.Task[None]] = []
        self.pairing = scenario.startswith('pair-')
        self.reliable = scenario == 'pair-reliable'
        self.host_sequence = 0
        self.peer_sequence = 0
        self.fragments = bytearray()
        self.events: list[int] = []
        self.objects: dict[str, dict[str, dict[str, Variant]]] = {
            ADAPTER: {'org.bluez.Adapter1': {'Powered': Variant('b', scenario != 'powered-off')}},
            DEVICE: {
                'org.bluez.Device1': {
                    'Paired': Variant(
                        'b', scenario != 'unpaired' and (not self.pairing or scenario == 'pair-prebonded')
                    ),
                    'Connected': Variant('b', scenario == 'already-connected'),
                    'ServicesResolved': Variant('b', False),
                }
            },
            SERVICE: {
                'org.bluez.GattService1': {
                    'UUID': Variant('s', '6a4e2800-667b-11e3-949a-0800200c9a66'),
                    'Device': Variant('o', DEVICE),
                    'Primary': Variant('b', True),
                }
            },
            RECEIVE: {
                CHARACTERISTIC: {
                    'UUID': Variant('s', '6a4e2810-667b-11e3-949a-0800200c9a66'),
                    'Service': Variant('o', SERVICE),
                    'Flags': Variant('as', ['notify']),
                    'Notifying': Variant('b', False),
                }
            },
            SEND: {
                CHARACTERISTIC: {
                    'UUID': Variant('s', '6a4e2820-667b-11e3-949a-0800200c9a66'),
                    'Service': Variant('o', SERVICE),
                    'Flags': Variant('as', ['write']),
                }
            },
        }

    def handle(self, message: Message) -> Message | bool | None:
        if message.message_type != MessageType.METHOD_CALL or message.destination != 'org.bluez':
            return None
        self.calls.append(f'{message.path}:{message.member}')
        if message.interface == 'org.freedesktop.DBus.ObjectManager' and message.member == 'GetManagedObjects':
            return Message.new_method_return(message, 'a{oa{sa{sv}}}', [self.objects])
        if message.interface == PROPERTIES and message.member == 'Get':
            interface, name = cast(tuple[str, str], tuple(message.body))
            return Message.new_method_return(message, 'v', [self.objects[str(message.path)][interface][name]])
        if message.interface == PROPERTIES and message.member == 'GetAll':
            return Message.new_method_return(message, 'a{sv}', [self.objects[str(message.path)][str(message.body[0])]])
        if message.path == DEVICE and message.interface == 'org.bluez.Device1' and message.member == 'Pair':
            if self.scenario in ('pair-cancel', 'pair-bond-timeout'):
                return True  # Keep the fake confirmation pending until cancellation/deadline.
            if self.scenario == 'pair-bond-failed':
                return Message.new_error(message, 'org.bluez.Error.AuthenticationRejected', 'Synthetic rejection')
            self.objects[DEVICE]['org.bluez.Device1']['Paired'] = Variant('b', True)
            self.objects[DEVICE]['org.bluez.Device1']['Connected'] = Variant('b', True)
            self.objects[DEVICE]['org.bluez.Device1']['ServicesResolved'] = Variant('b', True)
            return Message.new_method_return(message)
        if message.path == DEVICE and message.interface == 'org.bluez.Device1' and message.member == 'CancelPairing':
            return Message.new_method_return(message)
        if (
            message.path == DEVICE
            and message.interface == 'org.bluez.Device1'
            and message.member in ('Connect', 'Disconnect')
        ):
            if message.member == 'Connect' and self.scenario == 'connect-failed':
                return Message.new_error(message, 'org.bluez.Error.Failed', 'Synthetic connect failure')
            connected = message.member == 'Connect'
            properties = self.objects[DEVICE]['org.bluez.Device1']
            properties['Connected'] = Variant('b', connected)
            properties['ServicesResolved'] = Variant('b', connected)
            return Message.new_method_return(message)
        if (
            message.path == RECEIVE
            and message.interface == CHARACTERISTIC
            and message.member in ('StartNotify', 'StopNotify')
        ):
            return Message.new_method_return(message)
        if message.path == SEND and message.interface == CHARACTERISTIC and message.member == 'WriteValue':
            payload = bytes(cast(bytes, message.body[0]))
            options = cast(dict[str, Variant], message.body[1])
            if options.get('type') != Variant('s', 'request'):
                return Message.new_error(message, 'org.bluez.Error.Failed', 'Expected acknowledged write')
            self.writes.append(payload)
            self.tasks.append(asyncio.create_task(self.respond(payload)))
            return Message.new_method_return(message)
        return Message.new_error(message, 'org.bluez.Error.NotSupported', 'Unexpected fake BlueZ operation')

    async def notify(self, payload: bytes) -> None:
        send = cast(Callable[[Message], Awaitable[object]], self.bus.send)
        await send(
            Message.new_signal(
                RECEIVE,
                PROPERTIES,
                'PropertiesChanged',
                'sa{sv}as',
                [CHARACTERISTIC, {'Value': Variant('ay', payload)}, []],
            )
        )

    async def respond(self, payload: bytes) -> None:
        await asyncio.sleep(0.01)
        if len(payload) == 13 and payload[:2] == b'\x00\x00':
            await self.notify(b'\x00\x01' + payload[2:12] + bytes([0, 0x86, int(not self.pairing or self.reliable)]))
            if self.scenario != 'identity-timeout':
                if self.pairing:
                    await self.pair_emit(IDENTITY)
                    await self.pair_notify(5050, b'\x01\xff')
                else:
                    for sequence, offset in enumerate(range(0, len(IDENTITY), 18)):
                        await self.notify(bytes([0xE0, sequence]) + IDENTITY[offset : offset + 18])
        elif len(payload) == 13 and payload[:2] == b'\x00\x02':
            if self.scenario not in ('close-timeout', 'pair-close-timeout'):
                await self.notify(b'\x00\x03' + payload[2:] + b'\x00')
        elif self.pairing and (payload[0] == 0x86 or self.reliable and payload[0] & 0x80):
            if self.reliable:
                if len(payload) == 2:
                    return
                if payload[1] & 63 != self.host_sequence:
                    raise AssertionError('Unexpected host MLR sequence')
                self.host_sequence = (self.host_sequence + 1) & 63
            for byte in payload[2 if self.reliable else 1 :]:
                if byte:
                    self.fragments.append(byte)
                elif self.fragments:
                    kind, body = unframe(bytes(self.fragments))
                    self.fragments.clear()
                    await self.pair_message(kind, body)
            if self.reliable:
                await self.notify(bytes([0xE0 | self.host_sequence >> 2, (self.host_sequence & 3) << 6]))
        elif payload != b'\xe1\x00':
            raise AssertionError(f'Unexpected protocol write: {payload.hex()}')

    async def pair_notify(self, kind: int, body: bytes) -> None:
        await self.pair_emit(frame(kind, body))

    async def pair_emit(self, encoded: bytes) -> None:
        size = 18 if self.reliable else 19
        for offset in range(0, len(encoded), size):
            prefix = b'\x86'
            if self.reliable:
                prefix = bytes([0xE0 | self.host_sequence >> 2, (self.host_sequence & 3) << 6 | self.peer_sequence])
                self.peer_sequence = (self.peer_sequence + 1) & 63
            await self.notify(prefix + encoded[offset : offset + size])

    async def pair_message(self, kind: int, body: bytes) -> None:
        if kind == 5000 and body[:3] in (b'\xa0\x13\x00', b'\xba\x13\x00'):
            return
        if kind == 0x8032:
            if body != bytes([10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 16]):
                raise AssertionError('Unexpected host capabilities')
            if self.scenario != 'pair-timeout':
                await self.pair_notify(0x8000, b'\xba\x13\x00')
            return
        expected = (4, 0, 14)
        index = len(self.events)
        if index >= 3 or kind != 0x811E + index * 256 or body != bytes([expected[index], 0]):
            raise AssertionError(f'Unexpected setup command: {kind:x}/{body.hex()}')
        self.events.append(body[0])
        await self.pair_notify(0x8100 + index * 256, b'\xa6\x13' + bytes([int(self.scenario == 'pair-rejected')]))


class BluezAcceptance(unittest.IsolatedAsyncioTestCase):
    binary: ClassVar[Path]
    output: ClassVar[Path]

    async def run_scenario(self, scenario: str) -> tuple[int, str, FakeBluez]:
        daemon = await asyncio.create_subprocess_exec(
            'dbus-daemon',
            '--config-file=' + str(Path(__file__).with_name('gfdi-smoke-bus.xml')),
            '--nofork',
            '--print-address=1',
            stdout=asyncio.subprocess.PIPE,
        )
        self.assertIsNotNone(daemon.stdout)
        assert daemon.stdout is not None
        bus: MessageBus | None = None
        child: asyncio.subprocess.Process | None = None
        fake: FakeBluez | None = None
        try:
            address = (await asyncio.wait_for(daemon.stdout.readline(), 5)).decode().strip()
            if not address:
                raise RuntimeError('Private D-Bus daemon exited before reporting its address')
            bus = await MessageBus(bus_address=address).connect()
            await bus.request_name('org.bluez')
            fake = FakeBluez(bus, scenario)
            bus.add_message_handler(fake.handle)
            environment = os.environ.copy()
            environment['DBUS_SYSTEM_BUS_ADDRESS'] = address
            mode = ['pair'] + ([] if fake.reliable else ['--plain']) if fake.pairing else ['live']
            child = await asyncio.create_subprocess_exec(
                str(self.binary),
                *mode,
                '--adapter',
                'hci0',
                '--device',
                'AA:BB:CC:DD:EE:FF',
                '--capture-dir',
                str(self.output / scenario),
                '--timeout-secs',
                '1',
                env=environment,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )
            if scenario == 'pair-cancel':
                async with asyncio.timeout(2):
                    while DEVICE + ':Pair' not in fake.calls:
                        await asyncio.sleep(0.01)
                child.send_signal(signal.SIGINT)
            stdout, _ = await asyncio.wait_for(child.communicate(), 10)
            await asyncio.gather(*fake.tasks)
            allowed = {
                'Get',
                'GetAll',
                'GetManagedObjects',
                'Connect',
                'Disconnect',
                'StartNotify',
                'StopNotify',
                'WriteValue',
            }
            if fake.pairing:
                allowed.add('Pair')
                allowed.add('CancelPairing')
            unexpected = [call for call in fake.calls if call.rsplit(':', 1)[-1] not in allowed]
            self.assertEqual(unexpected, [])
            assert child.returncode is not None
            return child.returncode, stdout.decode(), fake
        finally:
            if child is not None and child.returncode is None:
                child.kill()
                await child.wait()
            if fake is not None:
                for task in fake.tasks:
                    task.cancel()
                await asyncio.gather(*fake.tasks, return_exceptions=True)
            if bus is not None:
                bus.disconnect()
                await bus.wait_for_disconnect()
            if daemon.returncode is None:
                daemon.terminate()
            await daemon.wait()

    async def test_identity_and_owned_cleanup(self) -> None:
        code, stdout, fake = await self.run_scenario('success')
        self.assertEqual(code, 0, stdout)
        self.assertEqual(json.loads(stdout)['model'], 'Demo tracker')
        self.assertEqual([len(payload) for payload in fake.writes], [13, 2, 13])
        register, ack, close = fake.writes
        self.assertEqual(register[:2], b'\x00\x00')
        self.assertEqual(ack, b'\xe1\x00')
        self.assertEqual(close, b'\x00\x02' + register[2:12] + b'\x86')
        self.assertIn(RECEIVE + ':StartNotify', fake.calls)
        self.assertIn(DEVICE + ':Disconnect', fake.calls)
        self.assertEqual((self.output / 'success').stat().st_mode & 0o777, 0o700)

    async def test_preconditions_do_not_connect_or_disconnect(self) -> None:
        for scenario in ('powered-off', 'unpaired', 'already-connected'):
            with self.subTest(scenario=scenario):
                code, stdout, fake = await self.run_scenario(scenario)
                self.assertNotEqual(code, 0, stdout)
                self.assertNotIn(DEVICE + ':Connect', fake.calls)
                self.assertNotIn(DEVICE + ':Disconnect', fake.calls)
                self.assertEqual(fake.writes, [])

    async def test_failures_still_disconnect(self) -> None:
        for scenario in ('connect-failed', 'identity-timeout', 'close-timeout'):
            with self.subTest(scenario=scenario):
                code, stdout, fake = await self.run_scenario(scenario)
                self.assertNotEqual(code, 0, stdout)
                self.assertIn(DEVICE + ':Disconnect', fake.calls)
                if scenario != 'connect-failed':
                    self.assertEqual(fake.writes[-1][:2], b'\x00\x02')

    async def test_pairing_establishes_bond_then_completes_garmin_setup(self) -> None:
        for scenario in ('pair-success', 'pair-prebonded', 'pair-reliable'):
            with self.subTest(scenario=scenario):
                code, stdout, fake = await self.run_scenario(scenario)
                self.assertEqual(code, 0, stdout)
                self.assertEqual(fake.events, [4, 0, 14])
                self.assertEqual(DEVICE + ':Pair' in fake.calls, scenario != 'pair-prebonded')
                self.assertIn(DEVICE + ':Disconnect', fake.calls)
                self.assertEqual(fake.writes[-1][:2], b'\x00\x02')
                self.assertEqual(json.loads(stdout.splitlines()[-1])['model'], 'Demo tracker')

    async def test_pairing_failures_never_report_success_and_disconnect(self) -> None:
        for scenario in ('pair-bond-failed', 'pair-rejected', 'pair-timeout', 'pair-close-timeout'):
            with self.subTest(scenario=scenario):
                code, stdout, fake = await self.run_scenario(scenario)
                self.assertNotEqual(code, 0, stdout)
                self.assertIn(DEVICE + ':Disconnect', fake.calls)
                if scenario != 'pair-bond-failed':
                    self.assertEqual(fake.writes[-1][:2], b'\x00\x02')

    async def test_pending_pairing_is_cancelled_on_interrupt_and_deadline(self) -> None:
        for scenario in ('pair-cancel', 'pair-bond-timeout'):
            with self.subTest(scenario=scenario):
                code, stdout, fake = await self.run_scenario(scenario)
                self.assertNotEqual(code, 0, stdout)
                self.assertIn(DEVICE + ':CancelPairing', fake.calls)
                self.assertIn(DEVICE + ':Disconnect', fake.calls)
                self.assertEqual(fake.writes, [])


def main(binary: Path = ROOT / '.tmp/cargo-target/debug/examples/gfdi-smoke', output: Path | None = None) -> None:
    """Exercise actual bluer D-Bus calls without opening the system bus or a radio."""
    BluezAcceptance.binary = binary.resolve(strict=True)
    BluezAcceptance.output = output or ROOT / '.tmp' / ('gfdi-bluez-' + uuid.uuid4().hex)
    BluezAcceptance.output.mkdir(mode=0o700)
    print(f'Private BlueZ captures: {BluezAcceptance.output}', flush=True)
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(BluezAcceptance))
    if not result.wasSuccessful():
        raise SystemExit(1)


if __name__ == '__main__':
    tyro.cli(main)
