import assert from 'node:assert/strict';
import test from 'node:test';
import { installAutomation, launchAutomation } from '../../apps/garmin-hass/web/ui-automation.js';

function fixture() {
    let report = null;
    const listeners = {};
    const marks = [];
    const calls = [];
    const browser = {
        innerWidth: 1440,
        innerHeight: 900,
        devicePixelRatio: 2,
        document: {
            hidden: false,
            hasFocus: () => true,
            addEventListener: (name, fn) => {
                listeners[name] = fn;
            },
        },
        addEventListener: (name, fn) => {
            listeners[name] = fn;
        },
        performance: { now: () => 0, mark: (name, options) => marks.push([name, options.detail]) },
        setInterval: fn => {
            listeners.tick = fn;
            return 1;
        },
        clearInterval: () => {
            delete listeners.tick;
        },
    };
    const command = (operation, argument) => {
        calls.push([operation, argument]);
        if (operation === 'list') return JSON.stringify({ value: ['stationary-arrival'] });
        if (operation === 'start') {
            if (report?.state === 'running') return JSON.stringify({ error: 'already running' });
            if (argument !== 'stationary-arrival') return JSON.stringify({ error: 'unknown scenario' });
            report = { state: 'running', phase: 'setup' };
        }
        if (operation === 'cancel' && report?.state === 'running')
            report = { ...report, state: 'cancelled', failure: argument };
        return JSON.stringify({ value: operation === 'status' ? report : null });
    };
    installAutomation(command, browser);
    return {
        browser,
        listeners,
        marks,
        calls,
        complete: () => {
            report.state = 'passed';
        },
    };
}

test('bridge exposes named commands, metadata and terminal-only results without pacing gestures', () => {
    const f = fixture();
    const api = f.browser.garminAutomation;
    assert.deepEqual(api.list(), ['stationary-arrival']);
    assert.equal(api.status(), null);
    assert.throws(() => api.start('eval(arbitrary)'), /unknown/);
    assert.equal(api.start('stationary-arrival').state, 'running');
    assert.equal(api.result(), null);
    assert.throws(() => api.start('stationary-arrival'), /already/);
    f.complete();
    f.listeners.tick();
    assert.equal(f.listeners.tick, undefined);
    assert.equal(api.result().state, 'passed');
    assert.equal(api.result().environment.dpr, 2);
    assert.deepEqual([...new Set(f.calls.map(([op]) => op))].sort(), ['list', 'start', 'status']);
    assert.equal(f.marks.at(-1)[1].state, 'passed');
});

test('visibility and focus loss cancel immediately even when frame scheduling is suspended', () => {
    for (const event of ['visibilitychange', 'blur']) {
        const f = fixture();
        f.browser.garminAutomation.start('stationary-arrival');
        f.browser.document.hidden = true;
        f.listeners[event]();
        assert.equal(f.browser.garminAutomation.result().state, 'cancelled');
        assert.throws(() => f.browser.garminAutomation.start('stationary-arrival'), /visible/);
    }
});

test('install is explicit and singleton', () => {
    const browser = {};
    assert.equal(browser.garminAutomation, undefined);
    const f = fixture();
    assert.deepEqual(Object.keys(f.listeners).sort(), ['blur', 'visibilitychange']);
    assert.throws(() => installAutomation(() => {}, f.browser), /already installed/);
});

test('watchdog cancels a run even if egui never produces another frame', () => {
    const f = fixture();
    f.browser.garminAutomation.start('stationary-arrival');
    f.browser.performance.now = () => 126000;
    f.listeners.tick();
    assert.equal(f.browser.garminAutomation.result().failure, 'scenario watchdog expired');
    assert.equal(f.listeners.tick, undefined);
});

test('the in-app launcher uses the API and records the same start metadata', () => {
    const f = fixture();
    launchAutomation('stationary-arrival', f.browser);
    assert.equal(f.browser.garminAutomation.status().state, 'running');
    assert.equal(f.marks[0][0], 'garmin.automation.start');
    assert.equal(f.marks[0][1].name, 'stationary-arrival');
    assert.throws(() => launchAutomation('unknown', f.browser), /already/);
});
