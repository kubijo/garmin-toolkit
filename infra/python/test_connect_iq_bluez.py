import asyncio
import shutil
import sys
import tempfile
import unittest
import uuid
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import cast
from unittest.mock import AsyncMock, patch

from dbus_fast import DBusError, Message, Variant
from dbus_fast.aio import MessageBus
from dbus_fast.annotations import DBusDict, DBusObjectPath
from dbus_fast.constants import MessageType
from dbus_fast.service import ServiceInterface, dbus_method

from connect_iq_bluez import OBJECT_MANAGER, PROPERTIES, BluezPeripheral, HostError
from connect_iq_gatt import (
    ADVERTISEMENT_PATH,
    ADVERTISING_MANAGER,
    APP_PATH,
    GATT_MANAGER,
    REQUEST_PATH,
    RESPONSE_PATH,
    SERVICE_PATH,
)
from connect_iq_host import Capture, serve
from connect_iq_protocol import REQUEST_UUID, RESPONSE_UUID, SERVICE_UUID, Snapshot
from test_connect_iq_host import frame


async def signal(bus: MessageBus, message: Message) -> None:
    # dbus-fast leaves the send future's result unparameterized.
    send = cast(Callable[[Message], Awaitable[object]], bus.send)
    await send(message)


class FakeGatt(ServiceInterface):
    def __init__(self, calls: list[str]) -> None:
        super().__init__(GATT_MANAGER)
        self.calls = calls
        self.active = False

    @dbus_method(name='RegisterApplication')
    def register(self, application: DBusObjectPath, _options: DBusDict) -> None:
        if application != APP_PATH:
            raise DBusError('org.bluez.Error.InvalidArguments', 'Unexpected application')
        self.calls.append('register-app')
        self.active = True

    @dbus_method(name='UnregisterApplication')
    def unregister(self, application: DBusObjectPath) -> None:
        if application != APP_PATH:
            raise DBusError('org.bluez.Error.InvalidArguments', 'Unexpected application')
        self.calls.append('unregister-app')
        self.active = False


class FakeAdvertising(ServiceInterface):
    def __init__(self, calls: list[str], gatt: FakeGatt) -> None:
        super().__init__(ADVERTISING_MANAGER)
        self.calls = calls
        self.gatt = gatt
        self.deny = False
        self.fail_remove = False
        self.active = False

    @dbus_method(name='RegisterAdvertisement')
    def register(self, advertisement: DBusObjectPath, _options: DBusDict) -> None:
        self.calls.append('register-ad')
        if advertisement != ADVERTISEMENT_PATH or not self.gatt.active or self.deny:
            raise DBusError('org.bluez.Error.NotPermitted', 'Synthetic registration failure')
        self.active = True

    @dbus_method(name='UnregisterAdvertisement')
    def unregister(self, advertisement: DBusObjectPath) -> None:
        self.calls.append('unregister-ad')
        if advertisement != ADVERTISEMENT_PATH or self.fail_remove:
            raise DBusError('org.bluez.Error.Failed', 'Synthetic cleanup failure')
        if not self.active:
            raise DBusError('org.bluez.Error.DoesNotExist', 'No advertisement')
        self.active = False


async def call(
    bus: MessageBus,
    destination: str,
    path: str,
    interface: str,
    member: str,
    signature: str = '',
    body: list[object] | None = None,
) -> Message:
    reply = await asyncio.wait_for(
        bus.call(
            Message(
                destination=destination,
                path=path,
                interface=interface,
                member=member,
                signature=signature,
                body=body or [],
            )
        ),
        timeout=2,
    )
    if reply.message_type == MessageType.ERROR:
        raise DBusError(reply.error_name or 'org.freedesktop.DBus.Error.Failed', str(reply.body))
    return reply


@unittest.skipUnless(sys.platform == 'linux' and shutil.which('dbus-daemon'), 'Linux/private dbus-daemon required')
class PrivateBusTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self.daemon = await asyncio.create_subprocess_exec(
            'dbus-daemon',
            '--session',
            '--nofork',
            '--nopidfile',
            '--print-address=1',
            '--address=unix:abstract=garmin-ciq-test-' + uuid.uuid4().hex,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        self.addAsyncCleanup(self.stop_daemon)
        assert self.daemon.stdout is not None
        address = (await asyncio.wait_for(self.daemon.stdout.readline(), timeout=5)).decode().strip()
        self.bluez = await MessageBus(bus_address=address).connect()
        self.addAsyncCleanup(self.close_bus, self.bluez)
        await self.bluez.request_name('org.bluez')
        self.calls: list[str] = []
        self.gatt = FakeGatt(self.calls)
        self.advertising = FakeAdvertising(self.calls, self.gatt)
        self.bluez.export('/org/bluez/hci0', self.gatt)
        self.bluez.export('/org/bluez/hci0', self.advertising)
        self.records: list[Snapshot] = []
        self.connection = await MessageBus(bus_address=address).connect()
        self.peripheral = BluezPeripheral(self.connection, 'hci0', self.records.append, 300)
        self.addAsyncCleanup(self.peripheral.close)
        self.other = await MessageBus(bus_address=address).connect()
        self.addAsyncCleanup(self.close_bus, self.other)
        destination = self.connection.unique_name
        assert destination is not None
        self.destination = destination
        self.peer = '/org/bluez/hci0/dev_SYNTHETIC_FIRST'

    async def stop_daemon(self) -> None:
        if self.daemon.returncode is None:
            self.daemon.terminate()
            try:
                await asyncio.wait_for(self.daemon.wait(), timeout=3)
            except TimeoutError:
                self.daemon.kill()
                await self.daemon.wait()

    async def close_bus(self, bus: MessageBus) -> None:
        bus.disconnect()
        await asyncio.wait_for(bus.wait_for_disconnect(), timeout=3)

    def options(self, peer: str | None = None) -> dict[str, Variant]:
        return {'device': Variant('o', peer or self.peer), 'offset': Variant('q', 0)}

    async def read_request(self, *, bus: MessageBus | None = None, options: dict[str, Variant] | None = None) -> bytes:
        result = await call(
            bus or self.bluez,
            self.destination,
            REQUEST_PATH,
            'org.bluez.GattCharacteristic1',
            'ReadValue',
            'a{sv}',
            [self.options() if options is None else options],
        )
        return bytes(result.body[0])

    async def write_response(self, payload: bytes, *, options: dict[str, Variant] | None = None) -> None:
        await call(
            self.bluez,
            self.destination,
            RESPONSE_PATH,
            'org.bluez.GattCharacteristic1',
            'WriteValue',
            'aya{sv}',
            [payload, self.options() if options is None else options],
        )

    async def test_real_wire_exports_and_full_snapshot_round_trip(self) -> None:
        await self.peripheral.start()
        managed = await call(self.other, self.destination, APP_PATH, OBJECT_MANAGER, 'GetManagedObjects')
        objects = managed.body[0]
        self.assertEqual(set(objects), {SERVICE_PATH, REQUEST_PATH, RESPONSE_PATH})
        self.assertEqual(objects[SERVICE_PATH]['org.bluez.GattService1']['UUID'].value, SERVICE_UUID)
        for path, expected_uuid, flags in (
            (REQUEST_PATH, REQUEST_UUID, ['read']),
            (RESPONSE_PATH, RESPONSE_UUID, ['write']),
        ):
            properties = objects[path]['org.bluez.GattCharacteristic1']
            self.assertEqual(properties['UUID'].value, expected_uuid)
            self.assertEqual(properties['Service'].value, SERVICE_PATH)
            self.assertEqual(properties['Flags'].value, flags)
        ad = await call(
            self.other, self.destination, ADVERTISEMENT_PATH, PROPERTIES, 'GetAll', 's', ['org.bluez.LEAdvertisement1']
        )
        self.assertEqual(ad.body[0]['LocalName'].value, 'TK Probe')
        self.assertEqual(ad.body[0]['Timeout'].value, 300)
        self.assertEqual(await self.read_request(), b'\x01\x01\0\0')
        self.assertEqual(await self.read_request(), b'\x01\x01\0\0')
        await self.write_response(frame())
        await self.write_response(frame())
        self.assertEqual(len(self.records), 1)
        self.assertEqual(self.records[0].steps, 1000)
        await self.peripheral.close()
        self.assertEqual(self.calls, ['register-app', 'register-ad', 'unregister-ad', 'unregister-app'])
        self.assertFalse(self.connection.connected)

    async def test_peer_isolation_offsets_and_non_bluez_callers_are_rejected(self) -> None:
        await self.peripheral.start()
        with self.assertRaises(DBusError):
            await self.read_request(bus=self.other)
        with self.assertRaises(DBusError):
            await self.read_request(options={})
        with self.assertRaises(DBusError):
            await self.read_request(options={'device': Variant('s', self.peer)})
        await self.read_request()
        with self.assertRaises(DBusError):
            await self.read_request(options=self.options('/org/bluez/hci0/dev_SYNTHETIC_OTHER'))
        with self.assertRaises(DBusError):
            await self.write_response(frame()[:-1])
        options = self.options()
        options['offset'] = Variant('q', 1)
        with self.assertRaises(DBusError):
            await self.write_response(frame(), options=options)
        options = self.options()
        options['type'] = Variant('s', 'command')
        with self.assertRaises(DBusError):
            await self.write_response(frame(), options=options)
        self.assertEqual(self.records, [])
        await self.write_response(frame())
        self.assertEqual(len(self.records), 1)

    async def test_disconnect_signal_releases_the_lease_and_rejects_stale_data(self) -> None:
        await self.peripheral.start()
        await self.read_request()
        await signal(
            self.bluez,
            Message.new_signal(
                self.peer,
                PROPERTIES,
                'PropertiesChanged',
                'sa{sv}as',
                ['org.bluez.Device1', {'Connected': Variant('b', False)}, []],
            ),
        )
        await self.read_request(options=self.options('/org/bluez/hci0/dev_SYNTHETIC_OTHER'))
        with self.assertRaises(DBusError):
            await self.write_response(frame())
        await self.write_response(frame(1), options=self.options('/org/bluez/hci0/dev_SYNTHETIC_OTHER'))
        self.assertEqual(len(self.records), 1)

    async def test_advertising_failure_rolls_back_application_registration(self) -> None:
        self.advertising.deny = True
        with self.assertRaises(HostError):
            await self.peripheral.start()
        await self.peripheral.close()
        self.assertFalse(self.gatt.active)
        self.assertFalse(self.advertising.active)
        self.assertFalse(self.connection.connected)

    async def test_removed_battery_interface_does_not_release_the_device_lease(self) -> None:
        await self.peripheral.start()
        await self.read_request()
        await signal(
            self.bluez,
            Message.new_signal('/', OBJECT_MANAGER, 'InterfacesRemoved', 'oas', [self.peer, ['org.bluez.Battery1']]),
        )
        with self.assertRaises(DBusError):
            await self.read_request(options=self.options('/org/bluez/hci0/dev_SYNTHETIC_OTHER'))
        await signal(
            self.bluez,
            Message.new_signal('/', OBJECT_MANAGER, 'InterfacesRemoved', 'oas', [self.peer, ['org.bluez.Device1']]),
        )
        self.assertEqual(
            await self.read_request(options=self.options('/org/bluez/hci0/dev_SYNTHETIC_OTHER')), b'\x01\x01\x01\0'
        )

    async def test_release_stops_admission_without_duplicate_unregistration(self) -> None:
        await self.peripheral.start()
        self.advertising.active = False
        await call(self.bluez, self.destination, ADVERTISEMENT_PATH, 'org.bluez.LEAdvertisement1', 'Release')
        self.assertTrue(self.peripheral.stopped.is_set())
        with self.assertRaises(DBusError):
            await self.read_request()
        await self.peripheral.close()
        self.assertEqual(self.calls, ['register-app', 'register-ad', 'unregister-app'])

    async def test_unregister_failure_still_removes_application_and_closes_bus(self) -> None:
        await self.peripheral.start()
        self.advertising.fail_remove = True
        with self.assertRaises(HostError):
            await self.peripheral.close()
        self.assertFalse(self.gatt.active)
        self.assertFalse(self.connection.connected)

    async def test_bluez_owner_loss_stops_admission(self) -> None:
        await self.peripheral.start()
        await self.bluez.release_name('org.bluez')
        await asyncio.wait_for(self.peripheral.stopped.wait(), timeout=2)
        self.assertFalse(self.peripheral.session.active)
        self.assertIsNotNone(self.peripheral.reason)

    async def test_missing_bluez_never_registers_or_leaves_a_bus_connection(self) -> None:
        await self.bluez.release_name('org.bluez')
        with self.assertRaises(HostError):
            await self.peripheral.start()
        await self.peripheral.close()
        self.assertEqual(self.calls, [])
        self.assertFalse(self.connection.connected)

    async def test_host_deadline_preserves_the_shared_disconnect_future_for_cleanup(self) -> None:
        with (
            tempfile.TemporaryDirectory() as temporary,
            Capture(Path(temporary) / 'capture') as capture,
            patch('connect_iq_host.MessageBus') as factory,
            patch('connect_iq_host.OUTPUT.panel'),
        ):
            factory.return_value.connect = AsyncMock(return_value=self.connection)
            await asyncio.wait_for(serve('hci0', 1, capture), timeout=5)
        self.assertFalse(self.connection.connected)
        self.assertEqual(self.calls, ['register-app', 'register-ad', 'unregister-ad', 'unregister-app'])

    async def test_cancelling_host_cleans_up_before_propagating_cancellation(self) -> None:
        ready = asyncio.Event()

        def panel(_body: str, *, title: str) -> None:
            if title == 'Telemetry host advertising':
                ready.set()

        with (
            tempfile.TemporaryDirectory() as temporary,
            Capture(Path(temporary) / 'capture') as capture,
            patch('connect_iq_host.MessageBus') as factory,
            patch('connect_iq_host.OUTPUT.panel', side_effect=panel),
        ):
            factory.return_value.connect = AsyncMock(return_value=self.connection)
            task = asyncio.create_task(serve('hci0', 30, capture))
            await asyncio.wait_for(ready.wait(), timeout=2)
            # A bus round trip lets the disconnect observer start before cancellation.
            await call(self.other, self.destination, APP_PATH, OBJECT_MANAGER, 'GetManagedObjects')
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
        self.assertFalse(self.connection.connected)
        self.assertEqual(self.calls, ['register-app', 'register-ad', 'unregister-ad', 'unregister-app'])
