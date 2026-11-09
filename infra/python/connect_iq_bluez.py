"""Owned BlueZ application/advertisement lifecycle; no connection or pairing calls."""

import asyncio
import re
from collections.abc import Callable
from typing import cast

from dbus_fast import Message, Variant
from dbus_fast.aio import MessageBus
from dbus_fast.constants import MessageType
from dbus_fast.service import ServiceInterface

from connect_iq_gatt import (
    ADVERTISEMENT_PATH,
    ADVERTISING_MANAGER,
    APP_PATH,
    GATT_MANAGER,
    REQUEST_PATH,
    RESPONSE_PATH,
    SERVICE_PATH,
    Advertisement,
    ProbeService,
    RequestCharacteristic,
    ResponseCharacteristic,
)
from connect_iq_protocol import Snapshot
from connect_iq_session import ProbeSession

OBJECT_MANAGER = 'org.freedesktop.DBus.ObjectManager'
PROPERTIES = 'org.freedesktop.DBus.Properties'
BUS = 'org.freedesktop.DBus'


class HostError(ValueError):
    pass


class BluezPeripheral:
    """Requires a dedicated bus connection; close always releases that connection."""

    def __init__(self, bus: MessageBus, adapter: str, retain: Callable[[Snapshot], None], duration: int) -> None:
        if not re.fullmatch(r'hci[0-9]+', adapter) or not 1 <= duration <= 3600:
            raise ValueError('Select an hciN adapter and a duration from 1 to 3600 seconds')
        self.bus = bus
        self.adapter = '/org/bluez/' + adapter
        self.session = ProbeSession(retain)
        self.stopped = asyncio.Event()
        self.reason: str | None = None
        self._owner: str | None = None
        self._app_attempted = False
        self._ad_attempted = False
        self._ad_released = False
        self._closed = False
        self._started = False
        self._exports: dict[str, ServiceInterface] = {
            SERVICE_PATH: ProbeService(),
            REQUEST_PATH: RequestCharacteristic(self.session),
            RESPONSE_PATH: ResponseCharacteristic(self.session),
            ADVERTISEMENT_PATH: Advertisement(duration, self._released),
        }

    async def _call(
        self,
        destination: str,
        path: str,
        interface: str,
        member: str,
        signature: str = '',
        body: list[object] | None = None,
    ) -> Message:
        reply = await asyncio.wait_for(
            self.bus.call(
                Message(
                    destination=destination,
                    path=path,
                    interface=interface,
                    member=member,
                    signature=signature,
                    body=body or [],
                )
            ),
            timeout=5,
        )
        if reply.message_type == MessageType.ERROR:
            raise HostError(member + ': ' + str(reply.error_name))
        return reply

    async def start(self) -> None:
        if self._started or self._closed:
            raise HostError('Create a fresh peripheral and bus connection for each run')
        self._started = True
        self.bus.add_message_handler(self._message)
        await self._call(
            BUS,
            '/org/freedesktop/DBus',
            BUS,
            'AddMatch',
            's',
            [
                "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.bluez'"
            ],
        )
        reply = await self._call(BUS, '/org/freedesktop/DBus', BUS, 'GetNameOwner', 's', ['org.bluez'])
        if len(reply.body) != 1 or not isinstance(reply.body[0], str):
            raise HostError('BlueZ did not provide its bus identity')
        owner: str = reply.body[0]
        self._owner = owner
        for rule in (
            "type='signal',sender='org.bluez',interface='org.freedesktop.DBus.Properties',path_namespace='"
            + self.adapter
            + "'",
            "type='signal',sender='org.bluez',interface='org.freedesktop.DBus.ObjectManager',member='InterfacesRemoved'",
        ):
            await self._call(BUS, '/org/freedesktop/DBus', BUS, 'AddMatch', 's', [rule])
        self._check_live()
        for path, interface in self._exports.items():
            self.bus.export(path, interface)
        self._app_attempted = True
        await self._call(owner, self.adapter, GATT_MANAGER, 'RegisterApplication', 'oa{sv}', [APP_PATH, {}])
        self._check_live()
        self.session.active = True
        self._ad_attempted = True
        await self._call(
            owner, self.adapter, ADVERTISING_MANAGER, 'RegisterAdvertisement', 'oa{sv}', [ADVERTISEMENT_PATH, {}]
        )
        self._check_live()

    def _check_live(self) -> None:
        if self.stopped.is_set():
            raise HostError(self.reason or 'Advertisement released during registration')

    def _released(self) -> None:
        self._ad_released = True
        self.session.close()
        self.stopped.set()

    def _message(self, message: Message) -> Message | None:
        if message.message_type == MessageType.METHOD_CALL:
            if (
                message.path in (REQUEST_PATH, RESPONSE_PATH, ADVERTISEMENT_PATH)
                and message.member in ('ReadValue', 'WriteValue', 'Release')
                and message.sender != self._owner
            ):
                return Message.new_error(
                    message, 'org.freedesktop.DBus.Error.AccessDenied', 'Only BlueZ may deliver peripheral operations'
                )
            return None
        if message.message_type != MessageType.SIGNAL:
            return None
        body = message.body
        if (
            message.sender == BUS
            and message.interface == BUS
            and message.member == 'NameOwnerChanged'
            and len(body) == 3
            and body[0] == 'org.bluez'
            and body[2] != self._owner
        ):
            self.reason = 'BlueZ ownership changed; restart the telemetry host'
            self.session.close()
            self.stopped.set()
        elif message.sender == self._owner:
            if (
                message.interface == PROPERTIES
                and message.member == 'PropertiesChanged'
                and len(body) == 3
                and body[0] == 'org.bluez.Device1'
                and isinstance(body[1], dict)
            ):
                connected = cast(dict[str, object], body[1]).get('Connected')
                if isinstance(connected, Variant) and connected.value is False and message.path is not None:
                    self.session.disconnect(message.path)
            elif (
                message.interface == OBJECT_MANAGER
                and message.member == 'InterfacesRemoved'
                and len(body) == 2
                and isinstance(body[0], str)
                and isinstance(body[1], list)
            ):
                if body[0] == self.adapter and 'org.bluez.Adapter1' in body[1]:
                    self.reason = 'Selected Bluetooth adapter was removed'
                    self.session.close()
                    self.stopped.set()
                elif 'org.bluez.Device1' in body[1]:
                    self.session.disconnect(body[0])
        return None

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self.session.close()
        failures: list[Exception] = []
        try:
            for attempted, interface, member, path in (
                (
                    self._ad_attempted and not self._ad_released,
                    ADVERTISING_MANAGER,
                    'UnregisterAdvertisement',
                    ADVERTISEMENT_PATH,
                ),
                (self._app_attempted, GATT_MANAGER, 'UnregisterApplication', APP_PATH),
            ):
                if attempted and self.bus.connected:
                    try:
                        await self._call(self._owner or 'org.bluez', self.adapter, interface, member, 'o', [path])
                    except (HostError, OSError) as error:
                        if not str(error).endswith(
                            ('DoesNotExist', 'ServiceUnknown', 'NameHasNoOwner', 'UnknownObject')
                        ):
                            failures.append(error)
        finally:
            self.bus.remove_message_handler(self._message)
            for path in self._exports:
                self.bus.unexport(path)
            self.bus.disconnect()
            await asyncio.wait_for(asyncio.shield(self.bus.wait_for_disconnect()), timeout=5)
        if failures:
            raise HostError('BlueZ unregister failed; the owned bus connection was closed') from ExceptionGroup(
                'BlueZ cleanup errors', failures
            )
