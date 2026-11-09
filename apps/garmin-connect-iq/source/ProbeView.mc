import Toybox.Application;
import Toybox.Graphics;
import Toybox.Math;
import Toybox.Timer;
import Toybox.WatchUi;

class ProbeView extends WatchUi.View {
    const ACCENT = 0x18B8D8;
    const MUTED = 0xA0A8B0;
    const TRACK = 0x303840;
    hidden var _refresh;
    hidden var _frame = 0;
    hidden var _surface = null;
    hidden var _surfaceWidth = 0;
    hidden var _surfaceHeight = 0;

    function initialize() {
        View.initialize();
        _refresh = new Timer.Timer();
    }

    function onShow() {
        _refresh.start(method(:refresh), 125, true);
    }

    function onHide() {
        _refresh.stop();
    }

    function refresh() {
        _frame = (_frame + 1) % 8;
        WatchUi.requestUpdate();
    }

    function spinner(dc, x, y) {
        dc.setPenWidth(2);
        for (var index = 0; index < 8; index += 1) {
            var angle = (index * Math.PI) / 4;
            dc.setColor(index == _frame ? ACCENT : TRACK, Graphics.COLOR_BLACK);
            dc.drawLine(
                x + Math.cos(angle) * 8,
                y + Math.sin(angle) * 8,
                x + Math.cos(angle) * 12,
                y + Math.sin(angle) * 12
            );
        }
        dc.setPenWidth(1);
    }

    function reading(value, suffix) {
        return value == null ? "n/a" : value.toString() + suffix;
    }

    function row(dc, x, y, width, font, label, value) {
        dc.setColor(MUTED, Graphics.COLOR_BLACK);
        dc.drawText(x, y, font, label, Graphics.TEXT_JUSTIFY_LEFT);
        dc.setColor(Graphics.COLOR_WHITE, Graphics.COLOR_BLACK);
        dc.drawText(x + width, y, font, value, Graphics.TEXT_JUSTIFY_RIGHT);
    }

    function onUpdate(dc) {
        if (Graphics has :createBufferedBitmap) {
            var bitmap = _surface == null ? null : _surface.get();
            if (bitmap == null || _surfaceWidth != dc.getWidth() || _surfaceHeight != dc.getHeight()) {
                _surfaceWidth = dc.getWidth();
                _surfaceHeight = dc.getHeight();
                _surface = Graphics.createBufferedBitmap({:width => _surfaceWidth, :height => _surfaceHeight});
                bitmap = _surface.get();
            }
            if (bitmap != null) {
                // Commit a complete frame instead of exposing partially redrawn rows during animation.
                render(bitmap.getDc());
                dc.drawBitmap(0, 0, bitmap);
                return;
            }
        }
        render(dc);
    }

    function render(dc) {
        var transport = Application.getApp().transport;
        var compact = dc.getWidth() < 360 || dc.getHeight() < 400;
        var font = compact ? Graphics.FONT_XTINY : Graphics.FONT_TINY;
        var line = dc.getFontHeight(font);
        var gap = compact ? 4 : 8;
        var x = compact ? 32 : 24;
        var width = dc.getWidth() - 2 * x;
        var center = dc.getWidth() / 2;
        var y = compact ? 24 : 32;
        dc.setColor(Graphics.COLOR_WHITE, Graphics.COLOR_BLACK);
        dc.clear();
        dc.drawText(center, y, font, transport.BUILD, Graphics.TEXT_JUSTIFY_CENTER);
        if (transport.isWorking()) {
            var titleWidth = dc.getTextWidthInPixels(transport.BUILD, font);
            spinner(dc, center - titleWidth / 2 - 24, y + line / 2);
        }
        y += line + 8;
        dc.setColor(Graphics.COLOR_WHITE, Graphics.COLOR_BLACK);
        dc.drawText(center, y, font, transport.status, Graphics.TEXT_JUSTIFY_CENTER);
        // Reserve the same status and progress slots in every transport state.
        y += 3 * line + 8;

        var limit = transport.phaseLimit();
        if (limit > 0) {
            var elapsed = transport.phaseElapsed();
            dc.setColor(MUTED, Graphics.COLOR_BLACK);
            dc.drawText(
                center,
                y,
                font,
                elapsed.toString() + " s / " + limit + " s limit",
                Graphics.TEXT_JUSTIFY_CENTER
            );
            dc.setColor(TRACK, Graphics.COLOR_BLACK);
            dc.fillRectangle(x, y + line + gap, width, 8);
            dc.setColor(ACCENT, Graphics.COLOR_BLACK);
            var boundedElapsed = elapsed < limit ? elapsed : limit;
            dc.fillRectangle(x, y + line + gap, (width * boundedElapsed) / limit, 8);
        }
        y += line + gap + 16;
        dc.setColor(MUTED, Graphics.COLOR_BLACK);
        dc.drawText(center, y, font, transport.scanSummary(), Graphics.TEXT_JUSTIFY_CENTER);
        y += line + gap;
        var age = transport.lastSendAge();
        if (compact) {
            dc.drawText(
                center,
                y,
                font,
                "Sent " + transport.sentCount() + " / ACK " + reading(age, "s ago"),
                Graphics.TEXT_JUSTIFY_CENTER
            );
            y += line + gap;
        } else {
            row(dc, x, y, width, font, "Sent this launch", transport.sentCount().toString());
            y += line + gap;
            row(dc, x, y, width, font, "Last confirmed send", reading(age, " s ago"));
            y += line + 16;
        }
        var snapshot = transport.lastSnapshot();
        if (snapshot == null && compact) {
            dc.setColor(MUTED, Graphics.COLOR_BLACK);
            dc.drawText(
                center,
                y,
                font,
                "Waiting for first\nconfirmed snapshot",
                Graphics.TEXT_JUSTIFY_CENTER
            );
            return y + 2 * line;
        }
        if (compact) {
            dc.setColor(Graphics.COLOR_WHITE, Graphics.COLOR_BLACK);
            dc.drawText(
                center,
                y,
                font,
                "Battery " + reading(snapshot.battery, "%") + " / HR " + reading(snapshot.heartRate, ""),
                Graphics.TEXT_JUSTIFY_CENTER
            );
            y += line + gap;
            dc.drawText(
                center,
                y,
                font,
                "Steps " + reading(snapshot.steps, "") + " / Stress " + reading(snapshot.stress, ""),
                Graphics.TEXT_JUSTIFY_CENTER
            );
            y += line + gap;
            dc.drawText(center, y, font, "Sequence " + snapshot.sequence, Graphics.TEXT_JUSTIFY_CENTER);
        } else {
            dc.setColor(ACCENT, Graphics.COLOR_BLACK);
            dc.drawText(
                x,
                y,
                font,
                snapshot == null ? "Awaiting first telemetry" : "Last confirmed telemetry",
                Graphics.TEXT_JUSTIFY_LEFT
            );
            y += line + 8;
            row(dc, x, y, width, font, "Battery", reading(snapshot == null ? null : snapshot.battery, "%"));
            y += line + gap;
            row(
                dc,
                x,
                y,
                width,
                font,
                "Charging",
                snapshot == null || snapshot.charging == null ? "n/a" : snapshot.charging ? "Yes" : "No"
            );
            y += line + gap;
            row(dc, x, y, width, font, "Daily steps", reading(snapshot == null ? null : snapshot.steps, ""));
            y += line + gap;
            row(
                dc,
                x,
                y,
                width,
                font,
                "Heart rate",
                reading(snapshot == null ? null : snapshot.heartRate, " bpm")
            );
            y += line + gap;
            row(dc, x, y, width, font, "Stress", reading(snapshot == null ? null : snapshot.stress, ""));
            y += line + gap;
            row(dc, x, y, width, font, "Sequence", reading(snapshot == null ? null : snapshot.sequence, ""));
            y += line + gap;
            row(dc, x, y, width, font, "Timestamp", reading(snapshot == null ? null : snapshot.timestamp, ""));
            y += line + 8;
            dc.setColor(MUTED, Graphics.COLOR_BLACK);
            dc.drawText(x, y, font, "n/a = unavailable", Graphics.TEXT_JUSTIFY_LEFT);
        }
        return y + line;
    }
}
