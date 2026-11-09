import Toybox.ActivityMonitor;
import Toybox.Sensor;
import Toybox.System;
import Toybox.Time;

// Protocol v1: one 20-byte GATT write, all integers little-endian.
// Missing numbers use all-one bits; charging uses 255 when unavailable.
module Telemetry {
    function appendNumber(bytes, number, width) {
        var value = number;
        if (value == null) {
            value = -1;
        }
        for (var index = 0; index < width; index += 1) {
            bytes.add(value & 255);
            value >>= 8;
        }
    }

    function snapshot(sequence) {
        var stats = System.getSystemStats();
        var daily = null;
        if (Toybox has :ActivityMonitor && ActivityMonitor has :getInfo) {
            daily = ActivityMonitor.getInfo();
        }
        var sensor = null;
        if (Toybox has :Sensor && Sensor has :getInfo) {
            sensor = Sensor.getInfo();
        }
        return new ProbeSnapshot(
            sequence,
            Time.now().value(),
            stats.battery.toNumber(),
            stats has :charging ? stats.charging : null,
            daily != null && daily has :steps ? daily.steps : null,
            sensor != null && sensor has :heartRate ? sensor.heartRate : null,
            daily != null && daily has :stressScore ? daily.stressScore : null
        );
    }
}
