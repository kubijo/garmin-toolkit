import assert from 'node:assert/strict';
import test from 'node:test';
import { recordLog, bindLogs, receiveLog, postLog, installLogs } from '../../apps/garmin-hass/web/logging.js';

test('bootstrap events reach Rust once, then events bypass the bootstrap buffer', async () => {
    recordLog('Error', 'test', 'before WASM');
    const received = [];
    bindLogs(payload => received.push(JSON.parse(payload)));
    assert.deepEqual(received, [
        {
            kind: 'event',
            level: 'Error',
            component: 'test',
            message: 'before WASM',
            source: 'worker',
        },
    ]);
    recordLog('Info', 'test', 'after WASM', 'main');
    assert.equal(received.at(-1).message, 'after WASM');
    // A second Trunk/wasm-bindgen module copy must use the same Rust callback.
    const copy = await import('../../apps/garmin-hass/web/logging.js?second-copy');
    copy.recordLog('Info', 'test', 'other module');
    assert.equal(received.at(-1).message, 'other module');
    const replacement = [];
    bindLogs(payload => replacement.push(payload));
    assert.deepEqual(replacement, []);
});

test('worker envelopes reach Rust unchanged and unrelated messages remain untouched', () => {
    const received = [];
    bindLogs(payload => received.push(payload));
    const payload = JSON.stringify({
        kind: 'record',
        record: { session: 'worker', source_sequence: 7 },
    });
    assert.equal(receiveLog({ type: 'application-log', payload }), true);
    assert.deepEqual(received, [payload]);
    assert.equal(receiveLog([1, 'composition-ready']), false);
    assert.equal(receiveLog({ type: 'application-log', payload: null }), true);
    assert.equal(received.length, 1);
});

test('Rust worker records use postMessage without changing their payload', t => {
    const messages = [];
    const original = globalThis.postMessage;
    globalThis.postMessage = message => messages.push(message);
    t.after(() => {
        if (original === undefined) delete globalThis.postMessage;
        else globalThis.postMessage = original;
    });
    postLog('serialized Rust record');
    assert.deepEqual(messages, [{ type: 'application-log', payload: 'serialized Rust record' }]);
});

test('browser errors and promise rejections feed the Rust callback', () => {
    const listeners = {};
    const received = [];
    bindLogs(payload => received.push(JSON.parse(payload)));
    installLogs({
        addEventListener: (name, listener) => {
            listeners[name] = listener;
        },
    });
    listeners.error({ message: 'error event' });
    listeners.unhandledrejection({ reason: 'rejection event' });
    assert.deepEqual(
        received.map(event => event.message),
        ['error event', 'rejection event'],
    );
    assert.ok(received.every(event => event.level === 'Error' && event.component === 'javascript'));
});

test('workers forward errors even before WASM initializes', async t => {
    const key = Symbol.for('garmin.application-log-bridge');
    const previousBridge = globalThis[key];
    const previousPost = globalThis.postMessage;
    delete globalThis[key];
    const messages = [];
    globalThis.postMessage = message => messages.push(message);
    t.after(() => {
        globalThis[key] = previousBridge;
        if (previousPost === undefined) delete globalThis.postMessage;
        else globalThis.postMessage = previousPost;
    });
    const worker = await import('../../apps/garmin-hass/web/logging.js?worker-bootstrap');
    worker.recordLog('Error', 'map-worker', 'WASM initialization failed', 'preparation-worker');
    assert.equal(messages[0].type, 'application-log');
    assert.deepEqual(JSON.parse(messages[0].payload), {
        kind: 'event',
        level: 'Error',
        component: 'map-worker',
        message: 'WASM initialization failed',
        source: 'preparation-worker',
    });
    const received = [];
    worker.bindLogs(payload => received.push(payload));
    assert.deepEqual(received, []);
    worker.recordLog('Info', 'map-worker', 'WASM ready');
    assert.equal(received.length, 1);
    assert.equal(messages.length, 1);
});

test('page bootstrap capture is bounded and drained once when WASM initializes', async t => {
    const key = Symbol.for('garmin.application-log-bridge');
    const previousBridge = globalThis[key];
    const previousDocument = globalThis.document;
    delete globalThis[key];
    globalThis.document = {};
    t.after(() => {
        globalThis[key] = previousBridge;
        if (previousDocument === undefined) delete globalThis.document;
        else globalThis.document = previousDocument;
    });
    const page = await import('../../apps/garmin-hass/web/logging.js?page-bootstrap');
    for (let index = 0; index < 30; index++) page.recordLog('Error', 'startup', `failure ${index}`);
    const received = [];
    page.bindLogs(payload => received.push(JSON.parse(payload)));
    assert.equal(received.length, 16);
    assert.equal(received[0].source, 'main');
    assert.equal(received.at(-1).message, 'failure 15');
    page.bindLogs(() => assert.fail('bootstrap event replayed'));
});
