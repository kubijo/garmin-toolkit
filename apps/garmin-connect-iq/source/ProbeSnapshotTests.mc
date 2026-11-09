import Toybox.Application;
import Toybox.Graphics;
import Toybox.System;
import Toybox.Test;

(:test)
function snapshotGoldenFrame(logger) {
    var snapshot = new ProbeSnapshot(0x1234, 0x01020304, 75, true, 1000, 60, 25);
    var expected = [1, 1, 0x34, 0x12, 4, 3, 2, 1, 75, 0, 1, 0, 0xE8, 3, 0, 0, 60, 0, 25, 0]b;
    Test.assertEqual(snapshot.payload.size(), expected.size());
    for (var index = 0; index < expected.size(); index += 1) {
        Test.assertEqual(snapshot.payload[index], expected[index]);
    }
    return true;
}

(:test)
function snapshotUnavailableFields(logger) {
    var snapshot = new ProbeSnapshot(0, 0, null, null, null, null, null);
    Test.assertEqual(snapshot.payload.size(), 20);
    Test.assertEqual(snapshot.payload[11], 0);
    for (var index = 8; index < 20; index += 1) {
        if (index != 11) {
            Test.assertEqual(snapshot.payload[index], 255);
        }
    }
    return true;
}

(:test)
function snapshotDeviceCapabilities(logger) {
    var snapshot = Telemetry.snapshot(7);
    Test.assertEqual(snapshot.sequence, 7);
    Test.assertEqual(snapshot.payload.size(), 20);
    if (!(Toybox has :ActivityMonitor)) {
        Test.assert(snapshot.steps == null);
        Test.assert(snapshot.stress == null);
    }
    return true;
}

(:test)
function dashboardRendersDeviceStates(logger) {
    var settings = System.getDeviceSettings();
    var width = settings.screenWidth;
    var height = settings.screenHeight - (width >= 360 ? 96 : 0);
    var surface = Graphics.createBufferedBitmap({:width => width, :height => height});
    var bitmap = surface.get();
    Test.assert(bitmap != null);
    var dc = bitmap.getDc();
    var app = Application.getApp();
    var original = app.transport;
    try {
        var bottom = null;
        var modes = ["scanning", "connecting", "expired", "sent"];
        for (var index = 0; index < modes.size(); index += 1) {
            app.transport = new ProbeFixtureTransport(modes[index]);
            var renderedBottom = app.probeView.render(dc);
            Test.assert(renderedBottom <= height);
            if (bottom != null && width >= 360 && height >= 400) {
                Test.assertEqual(renderedBottom, bottom);
            }
            bottom = renderedBottom;
            app.probeView.onUpdate(dc);
        }
    } finally {
        app.transport = original;
    }
    return true;
}
