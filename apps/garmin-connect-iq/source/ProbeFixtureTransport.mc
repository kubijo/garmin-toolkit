(:test)
class ProbeFixtureTransport {
    const BUILD = "Render test";
    var status;
    hidden var _mode;
    hidden var _snapshot;

    function initialize(mode) {
        _mode = mode;
        status = mode == "scanning" ? "Scanning" : mode == "connecting" ? "Connecting" : "Telemetry sent";
        _snapshot = mode == "sent" ? new ProbeSnapshot(42, 1794224381, 99, false, null, 60, null) : null;
    }

    function isWorking() {
        return _mode != "sent";
    }

    function phaseLimit() {
        return _mode == "scanning" || _mode == "expired" ? 10 : _mode == "connecting" ? 60 : 0;
    }

    function phaseElapsed() {
        return _mode == "expired" ? 30 : 3;
    }

    function sentCount() {
        return _mode == "sent" ? 43 : 0;
    }

    function lastSendAge() {
        return _mode == "sent" ? 2 : null;
    }

    function lastSnapshot() {
        return _snapshot;
    }

    function scanSummary() {
        return "Packets 28 / Matches 1";
    }
}
