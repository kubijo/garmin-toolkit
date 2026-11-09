import Toybox.Lang;

class ProbeSnapshot {
    var sequence as Lang.Number;
    var timestamp as Lang.Number;
    var battery as Lang.Number?;
    var charging as Lang.Boolean?;
    var steps as Lang.Number?;
    var heartRate as Lang.Number?;
    var stress as Lang.Number?;
    var payload as Lang.ByteArray;

    function initialize(
        sequenceValue,
        timestampValue,
        batteryValue,
        chargingValue,
        stepsValue,
        heartRateValue,
        stressValue
    ) {
        sequence = sequenceValue;
        timestamp = timestampValue;
        battery = batteryValue;
        charging = chargingValue;
        steps = stepsValue;
        heartRate = heartRateValue;
        stress = stressValue;
        payload = []b;
        Telemetry.appendNumber(payload, 1, 1);
        Telemetry.appendNumber(payload, 1, 1);
        Telemetry.appendNumber(payload, sequence, 2);
        Telemetry.appendNumber(payload, timestamp, 4);
        Telemetry.appendNumber(payload, battery, 2);
        Telemetry.appendNumber(payload, charging == null ? null : charging ? 1 : 0, 1);
        Telemetry.appendNumber(payload, 0, 1);
        Telemetry.appendNumber(payload, steps, 4);
        Telemetry.appendNumber(payload, heartRate, 2);
        Telemetry.appendNumber(payload, stress, 2);
    }
}
