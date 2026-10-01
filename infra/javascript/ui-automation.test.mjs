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
    let background = false;
    let cleanup = false;
    let nextFrame = 0;
    let nextTimer = 0;
    const frames = new Map();
    const timeouts = new Map();
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
        requestAnimationFrame: callback => {
            frames.set(++nextFrame, callback);
            return nextFrame;
        },
        cancelAnimationFrame: id => frames.delete(id),
        setTimeout: callback => {
            timeouts.set(++nextTimer, callback);
            return nextTimer;
        },
        clearTimeout: id => timeouts.delete(id),
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
        let requestedBackground = false;
        if (['start', 'action', 'sequence'].includes(operation) && argument.startsWith('{')) {
            const request = JSON.parse(argument);
            if ('run_in_background' in request) {
                if (typeof request.run_in_background !== 'boolean')
                    return JSON.stringify({ error: 'invalid automation run options' });
                requestedBackground = request.run_in_background;
                argument = operation === 'start' ? request.argument : JSON.stringify(request.argument);
            }
        }
        if (operation === 'list') return JSON.stringify({ value: ['stationary-arrival'] });
        if (operation === 'start') {
            if (['running', 'paused'].includes(report?.state)) return JSON.stringify({ error: 'already running' });
            if (argument !== 'stationary-arrival') return JSON.stringify({ error: 'unknown scenario' });
            report = { state: 'running', phase: 'setup' };
            background = requestedBackground;
        }
        if (operation === 'pause' && report?.state === 'running') report.state = 'paused';
        if (operation === 'resume' && report?.state === 'paused') report.state = 'running';
        if (operation === 'cancel' && ['running', 'paused'].includes(report?.state))
            report = { ...report, state: 'cancelled', failure: argument };
        const status = report
            ? { ...report, needs_background_frames: background && (report.state === 'running' || cleanup) }
            : null;
        return JSON.stringify({ value: operation === 'status' ? status : null });
    };
    installAutomation(command, browser);
    return {
        browser,
        listeners,
        marks,
        calls,
        frames,
        timeouts,
        complete: (pendingCleanup = false) => {
            report.state = 'passed';
            cleanup = pendingCleanup;
        },
        settle: () => {
            cleanup = false;
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

test('background fallback wakes an already-pending frame while the page reports visible', () => {
    const f = fixture();
    const delivered = [];
    f.browser.performance.now = () => 1234;
    f.browser.requestAnimationFrame(timestamp => delivered.push(timestamp));
    const nativeCallback = [...f.frames.values()][0];
    assert.equal(f.timeouts.size, 0);
    f.browser.garminAutomation.start('stationary-arrival', { run_in_background: true });
    assert.equal(f.browser.document.hidden, false);
    assert.equal(f.timeouts.size, 1);
    [...f.timeouts.values()][0]();
    assert.deepEqual(delivered, [1234]);
    assert.equal(f.frames.size, 0);
    nativeCallback(1235);
    assert.deepEqual(delivered, [1234], 'a late native callback must not deliver the frame twice');
});

test('native frame delivery and explicit cancellation clear their timer fallback', () => {
    const f = fixture();
    f.browser.garminAutomation.start('stationary-arrival', { run_in_background: true });
    const delivered = [];
    f.browser.requestAnimationFrame(timestamp => delivered.push(timestamp));
    const delayedTimer = [...f.timeouts.values()][0];
    [...f.frames.values()][0](100);
    assert.deepEqual(delivered, [100]);
    assert.equal(f.timeouts.size, 0);
    delayedTimer();
    assert.deepEqual(delivered, [100]);

    const id = f.browser.requestAnimationFrame(timestamp => delivered.push(timestamp));
    const cancelledTimer = [...f.timeouts.values()][0];
    const cancelledNative = [...f.frames.values()][0];
    f.browser.cancelAnimationFrame(id);
    assert.equal(f.timeouts.size, 0);
    assert.equal(f.frames.size, 0);
    cancelledTimer();
    cancelledNative(101);
    assert.deepEqual(delivered, [100]);
});

test('background frame delivery continues through cleanup then restores ordinary scheduling', () => {
    const f = fixture();
    const api = f.browser.garminAutomation;
    api.start('stationary-arrival', { run_in_background: true });
    let delivered = 0;
    f.browser.requestAnimationFrame(() => {
        delivered++;
        f.browser.requestAnimationFrame(() => delivered++);
    });
    [...f.timeouts.values()][0]();
    assert.equal(delivered, 1);
    assert.equal(f.timeouts.size, 1, 'frames scheduled by callbacks also get a fallback');
    f.complete(true);
    f.listeners.tick();
    assert.equal(f.timeouts.size, 1);
    assert.ok(f.listeners.tick, 'the observer must wait for cleanup');
    f.settle();
    f.listeners.tick();
    assert.equal(f.timeouts.size, 0);
    assert.equal(f.listeners.tick, undefined);
    assert.equal(f.frames.size, 1, 'the original pending animation request remains intact');
    [...f.frames.values()][0](200);
    assert.equal(delivered, 2);
    api.start('stationary-arrival');
    f.browser.requestAnimationFrame(() => delivered++);
    assert.equal(f.timeouts.size, 0, 'a subsequent ordinary run must not inherit the fallback');
});

test('background execution is requested per run through HTTP without changing subsequent runs', () => {
    const f = fixture();
    const api = f.browser.garminAutomation;
    f.browser.document.hidden = true;
    const request = { argument: 'stationary-arrival', run_in_background: true };
    const response = JSON.parse(
        executeAutomation(JSON.stringify({ operation: 'start', argument: request }), f.browser),
    );
    assert.equal(response.value, null);
    assert.equal(api.status().state, 'running');
    f.listeners.visibilitychange();
    assert.equal(api.status().state, 'running');
    assert.throws(() => api.start('stationary-arrival', { run_in_background: 'yes' }), /invalid/);
    assert.throws(() => api.start('stationary-arrival'), /already/);
    f.listeners.visibilitychange();
    assert.equal(api.status().state, 'running');
    f.complete();
    api.start('stationary-arrival');
    assert.equal(api.status().state, 'paused');
    f.browser.document.hidden = false;
    f.listeners.visibilitychange();
    assert.equal(api.status().state, 'running');
});

test('browser actions and sequences pass background options on the same launch command', () => {
    for (const [operation, argument] of [
        ['action', { kind: 'click', target: 'profile.0' }],
        ['sequence', [{ kind: 'click', target: 'profile.0' }]],
    ]) {
        const f = fixture();
        f.browser.garminAutomation[operation](argument, { run_in_background: true });
        assert.deepEqual(f.calls[0], [operation, JSON.stringify({ run_in_background: true, argument })]);
        assert.ok(!f.calls.some(([op]) => op === 'pause'));
    }
});

test('background execution retains the watchdog and cancellation', () => {
    const f = fixture();
    const api = f.browser.garminAutomation;
    f.browser.document.hidden = true;
    api.start('stationary-arrival', { run_in_background: true });
    f.browser.performance.now = () => 126000;
    f.listeners.tick();
    assert.equal(api.result().state, 'cancelled');
    assert.match(api.result().failure, /watchdog/);
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
