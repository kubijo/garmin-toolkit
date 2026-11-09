"""Typed BlueZ service declarations for the project-owned probe protocol."""

from collections.abc import Callable
from typing import Annotated

from dbus_fast import DBusError, Variant
from dbus_fast.annotations import DBusBool, DBusBytes, DBusDict, DBusObjectPath, DBusSignature, DBusStr, DBusUInt16
from dbus_fast.constants import PropertyAccess
from dbus_fast.service import ServiceInterface, dbus_method, dbus_property

from connect_iq_protocol import REQUEST_UUID, RESPONSE_UUID, SERVICE_UUID
from connect_iq_session import PeerBusy, ProbeSession

APP_PATH = '/io/kubijo/GarminToolkit/Probe'
SERVICE_PATH = APP_PATH + '/service0'
REQUEST_PATH = SERVICE_PATH + '/request'
RESPONSE_PATH = SERVICE_PATH + '/response'
ADVERTISEMENT_PATH = '/io/kubijo/GarminToolkit/ProbeAdvertisement'
GATT_MANAGER = 'org.bluez.GattManager1'
ADVERTISING_MANAGER = 'org.bluez.LEAdvertisingManager1'


def option(options: dict[str, Variant], name: str, signature: str, default: object = None) -> object:
    if name not in options:
        return default
    if options[name].signature != signature:
        raise DBusError('org.bluez.Error.InvalidArguments', 'Unexpected BlueZ option type')
    return options[name].value


def peer(options: dict[str, Variant]) -> str:
    if option(options, 'offset', 'q', 0) != 0:
        raise DBusError('org.bluez.Error.InvalidOffset', 'Long telemetry reads/writes are unsupported')
    if option(options, 'prepare-authorize', 'b', False) is not False:
        raise DBusError('org.bluez.Error.NotSupported', 'Prepared telemetry writes are unsupported')
    if option(options, 'type', 's', 'request') != 'request':
        raise DBusError('org.bluez.Error.NotSupported', 'Telemetry writes require an ATT response')
    device = option(options, 'device', 'o')
    if not isinstance(device, str) or not device.startswith('/'):
        raise DBusError('org.bluez.Error.InvalidArguments', 'BlueZ must supply the requesting device')
    return device


class ProbeService(ServiceInterface):
    def __init__(self) -> None:
        super().__init__('org.bluez.GattService1')

    @dbus_property(access=PropertyAccess.READ, name='UUID')
    def uuid(self) -> DBusStr:
        return SERVICE_UUID

    @dbus_property(access=PropertyAccess.READ, name='Primary')
    def primary(self) -> DBusBool:
        return True


class Characteristic(ServiceInterface):
    def __init__(self, uuid: str, flags: list[str]) -> None:
        super().__init__('org.bluez.GattCharacteristic1')
        self._uuid = uuid
        self._flags = flags

    @dbus_property(access=PropertyAccess.READ, name='UUID')
    def uuid(self) -> DBusStr:
        return self._uuid

    @dbus_property(access=PropertyAccess.READ, name='Service')
    def service(self) -> DBusObjectPath:
        return SERVICE_PATH

    @dbus_property(access=PropertyAccess.READ, name='Flags')
    def flags(self) -> Annotated[list[str], DBusSignature('as')]:
        return self._flags


class RequestCharacteristic(Characteristic):
    def __init__(self, session: ProbeSession) -> None:
        super().__init__(REQUEST_UUID, ['read'])
        self._session = session

    @dbus_method(name='ReadValue')
    def read_value(self, options: DBusDict) -> DBusBytes:
        try:
            return self._session.request(peer(options))
        except PeerBusy as error:
            raise DBusError('org.bluez.Error.NotPermitted', str(error)) from error


class ResponseCharacteristic(Characteristic):
    def __init__(self, session: ProbeSession) -> None:
        super().__init__(RESPONSE_UUID, ['write'])
        self._session = session

    @dbus_method(name='WriteValue')
    def write_value(self, value: DBusBytes, options: DBusDict) -> None:
        try:
            device = peer(options)
            if len(value) != 20:
                raise DBusError('org.bluez.Error.InvalidValueLength', 'Telemetry response must be twenty bytes')
            self._session.response(device, value)
        except PeerBusy as error:
            raise DBusError('org.bluez.Error.NotPermitted', str(error)) from error
        except ValueError as error:
            raise DBusError('org.bluez.Error.InvalidArguments', str(error)) from error
        except OSError as error:
            raise DBusError('org.bluez.Error.Failed', 'Unable to retain telemetry; request remains pending') from error


class Advertisement(ServiceInterface):
    def __init__(self, duration: int, released: Callable[[], None]) -> None:
        super().__init__('org.bluez.LEAdvertisement1')
        self._duration = duration
        self._released = released

    @dbus_property(access=PropertyAccess.READ, name='Type')
    def kind(self) -> DBusStr:
        return 'peripheral'

    @dbus_property(access=PropertyAccess.READ, name='ServiceUUIDs')
    def services(self) -> Annotated[list[str], DBusSignature('as')]:
        return [SERVICE_UUID]

    @dbus_property(access=PropertyAccess.READ, name='LocalName')
    def local_name(self) -> DBusStr:
        return 'TK Probe'

    @dbus_property(access=PropertyAccess.READ, name='Timeout')
    def timeout(self) -> DBusUInt16:
        return self._duration

    @dbus_method(name='Release')
    def release(self) -> None:
        self._released()
