import assert from 'node:assert/strict';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
import { executeAutomation, installAutomation, launchAutomation } from '../../apps/garmin-hass/web/ui-automation.js';
import {
    registerWindow,
    unregisterWindow,
    installWindowControl,
    captureWindow,
    installWindowDiagnostics,
    publishObservation,
    refreshDiagnostics,
} from '../../apps/garmin-hass/web/window-control.js';

test('window observations forward through the owner and closed handles cannot publish', () => {
    const parent = fixture();
    const child = fixture();
    const observations = [];
    installWindowDiagnostics(json => observations.push(JSON.parse(json)), parent.browser);
    installWindowControl(async () => {}, parent.browser);
    installWindowControl(async () => {}, child.browser);
    child.browser.opener = parent.browser;
    registerWindow('files', JSON.stringify({ kind: 'device-files', title: 'Files' }), child.browser, parent.browser);
    refreshDiagnostics(parent.browser);
    const id = observations.find(item => item.fields.kind === 'device-files').window;
    const observation = { kind: 'automation', window: 'root', fields: { state: 'passed' }, removed: false };
    publishObservation(JSON.stringify(observation), child.browser);
    assert.equal(observations.at(-1).window, id);
    unregisterWindow('files', parent.browser);
    assert.equal(observations.at(-1).removed, true);
    const count = observations.length;
    publishObservation(JSON.stringify(observation), child.browser);
    assert.equal(observations.length, count);
});

test('window discovery, scoped dispatch and reload invalidate handles without touching root automation', async () => {
    const parent = fixture();
    const child = fixture();
    let focused = 0;
    child.browser.focus = () => {
        focused++;
    };
    child.browser.close = () => {
        child.browser.closed = true;
    };
    installWindowControl(async () => ['{}', runInNewContext('new Uint8Array([1, 2, 3])')], child.browser);
    registerWindow('files', JSON.stringify({ kind: 'device-files', title: 'Files' }), child.browser, parent.browser);
    const dispatch = request => JSON.parse(executeAutomation(JSON.stringify(request), parent.browser));
    const list = () => dispatch({ operation: 'windows' }).value;
    const id = list()[1].id;
    assert.equal(list()[1].ready, true);
    dispatch({ operation: 'window.focus', window: id });
    assert.equal(focused, 1);
    dispatch({ operation: 'targets', window: id });
    assert.equal(child.calls.at(-1)[0], 'targets');
    assert.equal(parent.calls.length, 0);
    const pixels = await captureWindow(id, 1000, parent.browser);
    assert.deepEqual(pixels, ['{}', new Uint8Array([1, 2, 3])]);
    assert.ok(pixels[1] instanceof Uint8Array);
    installWindowControl(async () => 'new pixels', child.browser);
    assert.match(dispatch({ operation: 'targets', window: id }).error, /stale/);
    const replacement = list()[1].id;
    assert.notEqual(replacement, id);
    dispatch({ operation: 'window.close', window: replacement });
    assert.equal(child.browser.closed, true);
    assert.equal(list().length, 1);
    assert.match(dispatch({ operation: 'targets', window: replacement }).error, /stale/);
});

test('a destroyed or unresponsive popup cannot leave a capture pending forever', async () => {
    for (const revoke of ['close', 'owner', 'reload', 'timeout']) {
        const parent = fixture();
        const child = fixture();
        installWindowControl(() => new Promise(() => {}), child.browser);
        registerWindow(
            'files',
            JSON.stringify({ kind: 'device-files', title: 'Files' }),
            child.browser,
            parent.browser,
        );
        const listed = JSON.parse(executeAutomation('{"operation":"windows"}', parent.browser)).value;
        const capture = captureWindow(listed[1].id, revoke === 'timeout' ? 1 : 1000, parent.browser);
        if (revoke === 'close') child.browser.closed = true;
        if (revoke === 'owner') unregisterWindow('files', parent.browser);
        if (revoke === 'reload') installWindowControl(async () => 'new', child.browser);
        await assert.rejects(capture, /stale|changed|deadline/);
    }
});

test('window screenshot rejects closure, owner revocation and reload while awaiting pixels', async () => {
    for (const revoke of ['close', 'owner', 'reload']) {
        const parent = fixture();
        const child = fixture();
        let complete;
        installWindowControl(
            () =>
                new Promise(resolve => {
                    complete = resolve;
                }),
            child.browser,
        );
        registerWindow(
            'files',
            JSON.stringify({ kind: 'device-files', title: 'Files' }),
            child.browser,
            parent.browser,
        );
        const listed = JSON.parse(executeAutomation('{"operation":"windows"}', parent.browser)).value;
        const capture = captureWindow(listed[1].id, 1000, parent.browser);
        if (revoke === 'close') child.browser.closed = true;
        if (revoke === 'owner') unregisterWindow('files', parent.browser);
        if (revoke === 'reload') installWindowControl(async () => 'new', child.browser);
        complete('old pixels');
        await assert.rejects(capture, /stale|changed/);
        assert.equal(parent.calls.length, 0);
    }
});

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

test('forwarded commands retain hook metadata, hidden-tab pauses, cancellation and results', () => {
    const f = fixture();
    const call = (operation, argument) =>
        JSON.parse(executeAutomation(JSON.stringify({ operation, argument }), f.browser));
    assert.deepEqual(call('list').value, ['stationary-arrival']);
    f.browser.document.hidden = true;
    assert.equal(call('start', 'stationary-arrival').value, null);
    const started = call('status').value;
    assert.equal(started.state, 'paused');
    assert.equal(started.environment.dpr, 2);
    assert.equal(call('result').value, null);
    call('cancel', 'cancelled by HTTP client');
    assert.equal(call('result').value.state, 'cancelled');
    assert.equal(call('result').value.failure, 'cancelled by HTTP client');
    assert.match(call('eval', 'arbitrary').error, /unsupported/);
    assert.match(JSON.parse(executeAutomation('{', f.browser)).error, /JSON/);
    assert.match(JSON.parse(executeAutomation('{"operation":"list"}', {})).error, /disabled/);
});
