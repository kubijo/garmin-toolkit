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
        if (operation === 'pause' && report?.state === 'running') report.state = 'paused';
        if (operation === 'resume' && report?.state === 'paused') report.state = 'running';
        if (operation === 'cancel' && ['running', 'paused'].includes(report?.state))
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

test('focus loss is harmless and hidden tabs pause without expiring the watchdog', () => {
    const f = fixture();
    f.browser.document.hasFocus = () => false;
    f.browser.garminAutomation.start('stationary-arrival');
    assert.equal(f.listeners.blur, undefined);
    f.browser.document.hidden = true;
    f.listeners.visibilitychange();
    assert.equal(f.browser.garminAutomation.status().state, 'paused');
    assert.equal(f.browser.garminAutomation.result(), null);
    f.browser.performance.now = () => 200000;
    f.listeners.tick();
    assert.equal(f.browser.garminAutomation.status().state, 'paused');
    f.browser.document.hidden = false;
    f.listeners.visibilitychange();
    f.listeners.tick();
    assert.equal(f.browser.garminAutomation.status().state, 'running');
    f.browser.garminAutomation.cancel();
    assert.equal(f.browser.garminAutomation.result().state, 'cancelled');
});

test('install is explicit and singleton', () => {
    const browser = {};
    assert.equal(browser.garminAutomation, undefined);
    const f = fixture();
    assert.deepEqual(Object.keys(f.listeners).sort(), ['visibilitychange']);
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

test('responsive sequences are forwarded to Rust without scheduling their actions in JavaScript', () => {
    const f = fixture();
    const actions = [
        { kind: 'resize', width: 720, height: 480 },
        { kind: 'assert_available', target: 'map.fit' },
        { kind: 'assert_value', target: 'activity.0', value: 'selected' },
    ];
    f.browser.garminAutomation.sequence(actions);
    assert.deepEqual(f.calls[0], ['sequence', JSON.stringify(actions)]);
    assert.equal(f.marks[0][1].name, 'custom-sequence');
});
