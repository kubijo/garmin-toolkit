using Toybox.BluetoothLowEnergy as Ble;

import Toybox.Lang;
import Toybox.System;
import Toybox.Timer;
import Toybox.WatchUi;

class ProbeTransport extends Ble.BleDelegate {
    const SERVICE = "59102400-299d-4b88-a1ce-668c821d87ca";
    const REQUEST = "59102401-299d-4b88-a1ce-668c821d87ca";
    const RESPONSE = "59102402-299d-4b88-a1ce-668c821d87ca";
    const BUILD = "Probe 11";
    var status = "Select / tap to scan";
    hidden var _timer;
    hidden var _poll;
    hidden var _results as Lang.Array<Ble.ScanResult> = [];
    hidden var _device = null;
    hidden var _request = null;
    hidden var _response = null;
    hidden var _busy = false;
    hidden var _registered = false;
    hidden var _lastSequence = null;
    hidden var _active = false;
    hidden var _scanning = false;
    hidden var _registering = false;
    hidden var _stage = "Connection";
    hidden var _scanPackets = 0;
    hidden var _scanNames = 0;
    hidden var _scanUuids = 0;
    hidden var _logLines = 0;
    hidden var _phaseAt = 0;
    hidden var _phaseLimit = 0;
    hidden var _sentCount = 0;
    hidden var _lastSentAt = null;
    hidden var _pendingSnapshot = null;
    hidden var _lastSnapshot = null;

    function initialize() {
        BleDelegate.initialize();
        _timer = new Timer.Timer();
        _poll = new Timer.Timer();
        log("Started; connection slots=" + Ble.getAvailableConnectionCount());
    }

    function log(message) {
        // Bound logging per app launch; never log sensor values, names or addresses.
        if (_logLines < 500) {
            System.println(BUILD + " t=" + System.getTimer() + " " + message);
            _logLines += 1;
        } else if (_logLines == 500) {
            System.println(BUILD + " log limit reached; reopen app for another run");
            _logLines += 1;
        }
    }

    function show(message) {
        log("Status: " + message);
        status = message;
        WatchUi.requestUpdate();
    }

    function isWorking() {
        return _active && (_registering || _scanning || _phaseLimit > 0 || _busy);
    }

    function phaseLimit() {
        return _phaseLimit;
    }

    function phaseElapsed() {
        return ((System.getTimer() - _phaseAt) / 1000).toNumber();
    }

    function sentCount() {
        return _sentCount;
    }

    function lastSendAge() {
        return _lastSentAt == null ? null : ((System.getTimer() - _lastSentAt) / 1000).toNumber();
    }

    function lastSnapshot() {
        return _lastSnapshot;
    }

    function scanSummary() {
        return "Packets " + _scanPackets + " / Matches " + _results.size();
    }

    function start() {
        if (_active) {
            return;
        }
        stop();
        _active = true;
        Ble.setDelegate(self);
        if (_registered) {
            scan();
        } else {
            _registering = true;
            _stage = "Registration";
            _phaseAt = System.getTimer();
            _phaseLimit = 10;
            show("Registering probe");
            _timer.start(method(:timeout), 10000, false);
            Ble.registerProfile({
                :uuid            => Ble.stringToUuid(SERVICE),
                :characteristics => [
                    {:uuid => Ble.stringToUuid(REQUEST)},
                    {:uuid => Ble.stringToUuid(RESPONSE)}
                ]
            });
        }
    }

    function onProfileRegister(uuid, result) {
        log("Profile status=" + result);
        if (!_active) {
            return;
        }
        _timer.stop();
        _registering = false;
        if (result != Ble.STATUS_SUCCESS) {
            stop();
            show("Profile registration failed");
            return;
        }
        _registered = true;
        scan();
    }

    function scan() {
        _phaseAt = System.getTimer();
        _phaseLimit = 10;
        show("Scanning");
        _results = [];
        _scanPackets = 0;
        _scanNames = 0;
        _scanUuids = 0;
        _scanning = true;
        Ble.setScanState(Ble.SCAN_STATE_SCANNING);
        _timer.start(method(:finishScan), 10000, false);
    }

    function onScanResults(results) {
        if (!_active || !_scanning) {
            return;
        }
        for (var item = results.next(); item != null; item = results.next()) {
            var result = item as Ble.ScanResult;
            _scanPackets += 1;
            if (_scanPackets <= 64) {
                log(
                    "Advertisement "
                        + _scanPackets
                        + " rssi="
                        + result.getRssi()
                        + " namePresent="
                        + (result.getDeviceName() != null)
                        + " probeName="
                        + (result.getDeviceName() == "TK Probe")
                );
            }
            if (result.getDeviceName() == "TK Probe") {
                _scanNames += 1;
            }
            // Names may arrive separately in scan responses; the custom service selects this probe.
            var services = result.getServiceUuids();
            for (var uuid = services.next(); uuid != null; uuid = services.next()) {
                _scanUuids += 1;
                if (_scanPackets <= 64) {
                    log("Advertised service=" + uuid.toString());
                }
                if (!uuid.equals(Ble.stringToUuid(SERVICE))) {
                    continue;
                }
                var duplicate = false;
                for (var index = 0; index < _results.size(); index += 1) {
                    duplicate = duplicate || result.isSameDevice(_results[index]);
                }
                if (!duplicate && _results.size() < 2) {
                    _results.add(result);
                }
            }
        }
    }

    function onScanStateChange(state, result) {
        log("Scan state=" + state + " status=" + result);
        if (_active && _scanning && result != Ble.STATUS_SUCCESS) {
            fail("Scan status " + result);
        }
    }

    function finishScan() {
        log(
            "Scan finished packets="
                + _scanPackets
                + " probeNames="
                + _scanNames
                + " uuids="
                + _scanUuids
                + " matches="
                + _results.size()
                + " slots="
                + Ble.getAvailableConnectionCount()
        );
        _scanning = false;
        Ble.setScanState(Ble.SCAN_STATE_OFF);
        if (_results.size() != 1) {
            var count = _results.size();
            stop();
            if (count == 0) {
                show(
                    "No probe found\nPackets "
                        + _scanPackets
                        + " Names "
                        + _scanNames
                        + "\nUUIDs "
                        + _scanUuids
                        + "\nSelect to retry"
                );
            } else {
                show("Multiple probes\nStop extra hosts\nSelect to retry");
            }
            return;
        }
        show("Connecting");
        _phaseAt = System.getTimer();
        _phaseLimit = 60;
        _stage = "Connection";
        _device = Ble.pairDevice(_results[0]);
        if (_device == null) {
            fail("Connection rejected");
            return;
        }
        // Firmware may ask the owner to confirm the open BLE connection.
        _timer.start(method(:timeout), 60000, false);
    }

    function onConnectedStateChanged(device, state) {
        log("Connection state=" + state + " active=" + _active + " selected=" + (device == _device));
        if (!_active || device != _device) {
            return;
        }
        if (state != Ble.CONNECTION_STATE_CONNECTED) {
            stop();
            show("Disconnected; select to retry");
            return;
        }
        _timer.stop();
        _phaseLimit = 0;
        var service = device.getService(Ble.stringToUuid(SERVICE));
        if (service != null) {
            _request = service.getCharacteristic(Ble.stringToUuid(REQUEST));
            _response = service.getCharacteristic(Ble.stringToUuid(RESPONSE));
        }
        if (_request == null || _response == null) {
            fail("Missing GATT service\nor characteristics");
            return;
        }
        show("Connected\nWaiting for request");
        _poll.start(method(:poll), 5000, true);
        poll();
    }

    function poll() {
        if (!_busy && _request != null) {
            _busy = true;
            _stage = "Read";
            _timer.start(method(:timeout), 10000, false);
            _request.requestRead();
        }
    }

    function onCharacteristicRead(characteristic, result, value) {
        log("Read status=" + result + " length=" + (value == null ? -1 : value.size()));
        if (!_active || !_busy || characteristic != _request) {
            return;
        }
        if (result != Ble.STATUS_SUCCESS) {
            fail("Read status " + result);
            return;
        }
        if (value == null || value.size() != 4 || value[0] != 1 || value[1] != 1) {
            fail("Invalid request frame");
            return;
        }
        var sequence = value[2] | (value[3] << 8);
        log("Request sequence=" + sequence);
        if (sequence == _lastSequence) {
            _busy = false;
            _timer.stop();
            return;
        }
        _stage = "Write";
        _timer.start(method(:timeout), 10000, false);
        _pendingSnapshot = Telemetry.snapshot(sequence);
        _response.requestWrite(_pendingSnapshot.payload, {:writeType => Ble.WRITE_TYPE_WITH_RESPONSE});
        _lastSequence = sequence;
    }

    function onCharacteristicWrite(characteristic, result) {
        log("Write status=" + result);
        if (!_active || !_busy || characteristic != _response) {
            return;
        }
        _timer.stop();
        _busy = false;
        if (result != Ble.STATUS_SUCCESS) {
            fail("Write status " + result);
        } else {
            _sentCount += 1;
            _lastSentAt = System.getTimer();
            _lastSnapshot = _pendingSnapshot;
            _pendingSnapshot = null;
            show("Telemetry sent");
        }
    }

    function timeout() {
        fail(_stage + " timed out");
    }

    function fail(message) {
        stop();
        show(message + "\nSelect to retry");
    }

    function stop() {
        if (_active || _device != null) {
            log("Stopping; releasePeripheral=" + (_device != null));
        }
        _active = false;
        _phaseLimit = 0;
        _scanning = false;
        _timer.stop();
        _registering = false;
        _poll.stop();
        Ble.setScanState(Ble.SCAN_STATE_OFF);
        var device = _device;
        _device = null;
        _request = null;
        _response = null;
        _busy = false;
        _lastSequence = null;
        _pendingSnapshot = null;
        if (device != null) {
            Ble.unpairDevice(device);
        }
    }
}
