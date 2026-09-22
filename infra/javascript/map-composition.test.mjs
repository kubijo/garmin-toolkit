import assert from 'node:assert/strict';
import test from 'node:test';

import { compositionMessage, decodeComposition, requestMapDraw } from '../../apps/garmin-hass/web/worker-codec.js';
import { installCompositionWorker } from '../../apps/garmin-hass/web/map-render-worker.js';
import { waitMilliseconds, nextMapFrame } from '../../apps/garmin-hass/web/map-clock.js';
import {
    experimentMode,
    placementStyle,
    startComposition,
    beginCompositionPass,
    beginCompositionPaint,
    paintedComposition,
    compositionStatus,
    disposeComposition,
    setMapRoute,
    setMapView,
    reportRendererStartup,
    rendererSnapshot,
    rendererDiagnostics,
    takeRendererDiagnostics,
    mapReadiness,
} from '../../apps/garmin-hass/web/map-experiment.js';

test.beforeEach(t => {
    t.mock.method(console, 'table', () => {});
});

test('worker GL is the default and URL selection respects host rollback', () => {
    assert.equal(experimentMode(false, ''), '');
    assert.equal(experimentMode(false, '?map-render-mode=worker-gl'), '');
    assert.equal(experimentMode(false, '?map-render-mode=invalid'), '');
    assert.equal(experimentMode(true, ''), 'worker-gl');
    for (const mode of ['main-gl', 'worker-gl', 'worker-webgpu']) {
        assert.equal(experimentMode(true, `?map-render-mode=${mode}`), mode);
    }
    assert.throws(() => experimentMode(true, '?map-render-mode=invalid'));
    assert.throws(() => experimentMode(true, '?map-render-mode=main-gl&map-render-mode=worker-gl'));
});

test('startup table distinguishes baseline, initializing worker, confirmed worker and failure', t => {
    const tables = [];
    t.mock.method(console, 'table', snapshot => tables.push(snapshot));
    reportRendererStartup('', 'Gl', 'Test UI adapter', false);
    assert.deepEqual(tables[0], {
        mode: 'main-gl',
        mapThread: 'main',
        mapBackend: 'WebGL2',
        state: 'ready',
        uiBackend: 'WebGL2',
        uiAdapter: 'Test UI adapter',
        uploadTelemetry: false,
        error: '',
    });
    reportRendererStartup('', 'BrowserWebGpu', 'Test UI adapter', true);
    assert.equal(tables[1].mode, 'main-webgpu');
    assert.equal(tables[1].mapBackend, 'WebGPU');
    reportRendererStartup('worker-gl', 'Gl', 'Test UI adapter', true);
    assert.equal(tables[2].state, 'initializing');
    assert.equal(tables[2].mapBackend, 'pending', 'a requested backend is not yet a confirmed backend');
    const b = browser(t);
    b.worker.send('composition-ready', 'Gl', 8192);
    assert.equal(tables.at(-1).mapBackend, 'WebGL2');
    assert.equal(tables.at(-1).mapThread, 'worker');
    assert.equal(tables.at(-1).state, 'ready');
    assert.equal(JSON.parse(rendererDiagnostics()).label, 'worker-gl · ready');
    assert.equal(JSON.parse(takeRendererDiagnostics()).label, 'worker-gl · ready');
    assert.equal(takeRendererDiagnostics(), '', 'idle frames have no diagnostic payload');
    b.worker.send('composition-failed', 'device lost');
    assert.equal(tables.at(-1).state, 'failed');
    assert.equal(rendererSnapshot().error, 'device lost');
    assert.match(JSON.parse(rendererDiagnostics()).detail, /device lost/);
    assert.equal(JSON.parse(takeRendererDiagnostics()).label, 'worker-gl · failed');
});

test('physical placement preserves clipped projection and accounts for canvas CSS scaling', () => {
    const placement = placementStyle([-80, 100, 800, 600], [0, 120, 720, 400], [1600, 1200], {
        left: 10,
        top: 20,
        width: 800,
        height: 600,
    });
    assert.deepEqual(placement, {
        left: 10,
        top: 80,
        width: 360,
        height: 200,
        canvasLeft: -40,
        canvasTop: -10,
        canvasWidth: 400,
        canvasHeight: 300,
        backingWidth: 800,
        backingHeight: 600,
    });
    assert.throws(() =>
        placementStyle([0, 0, Infinity, 5], [0, 0, 5, 5], [10, 10], { left: 0, top: 0, width: 10, height: 10 }),
    );
    assert.throws(() =>
        placementStyle([0, 0, 5, 5], [0, 0, 5, 5], [0, 10], { left: 0, top: 0, width: 10, height: 10 }),
    );
});

test('shared composition codec rejects malformed and oversized payloads', () => {
    for (const message of [
        null,
        {},
        [2, 'composition-size', 1, 20, 30],
        [1, 'composition-size', 0, 20, 30],
        [1, 'composition-size', 1, 0, 30],
        [1, 'composition-size', 1, 32769, 30],
        [1, 'composition-size', 1, 8192, 8192],
        [1, 'composition-size', 1, 1.5, 30],
        [1, 'composition-size', 1, NaN, 30],
        [1, 'composition-size', 1, 20, 30, 'trailing'],
        [1, 'composition-failed', 'x'.repeat(1025)],
        [1, 'composition-ready', 'Vulkan', 8192],
    ]) {
        assert.throws(() => decodeComposition(message));
    }
    assert.deepEqual(decodeComposition(compositionMessage('composition-size', 7, 200, 100)), {
        type: 'composition-size',
        id: 7,
        width: 200,
        height: 100,
    });
    assert.equal(decodeComposition(compositionMessage('composition-size', 8, 4096, 4096)).width, 4096);
});

function browser(t, map = false) {
    t.after(disposeComposition);
    const nodes = [];
    function element() {
        const node = {
            style: {},
            append() {},
            before() {},
            remove() {
                this.removed = true;
            },
            getAttribute: () => null,
            removeAttribute() {},
            setAttribute() {},
            transferControlToOffscreen: () => ({ transferredCanvas: true }),
            getBoundingClientRect: () => ({ left: 0, top: 0, width: 1000, height: 800 }),
        };
        nodes.push(node);
        return node;
    }
    const workers = [];
    class Worker {
        constructor(url, options) {
            this.url = url;
            this.options = options;
            this.messages = [];
            workers.push(this);
        }
        postMessage(message, transfer) {
            this.messages.push({ message, transfer });
        }
        terminate() {
            this.terminated = true;
        }
        send(type, ...data) {
            this.onmessage({ data: compositionMessage(type, ...data) });
        }
    }
    const events = new EventTarget();
    const document = {
        hidden: false,
        baseURI: 'https://example.test/api/hass/',
        createElement: element,
        querySelector: query => ({
            content: `./${query.includes('map-render-worker') ? 'map-render-worker-ABCDEFG2.js' : query.includes('map-worker') ? 'map-worker-ABCDEFG2.js' : query.includes('wasm') ? 'app-0123456789abcdef.wasm' : 'app-ABCDEFG2.js'}`,
        }),
        addEventListener: events.addEventListener.bind(events),
        removeEventListener: events.removeEventListener.bind(events),
    };
    for (const [key, value] of Object.entries({ document, window: {}, Worker })) {
        const original = Object.getOwnPropertyDescriptor(globalThis, key);
        Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
        t.after(() => (original ? Object.defineProperty(globalThis, key, original) : delete globalThis[key]));
    }
    t.mock.method(console, 'error', () => {});
    const canvas = element();
    let repaints = 0;
    startComposition(canvas, 'worker-gl', () => repaints++, map);
    const draw = async (width = 600, height = 400, view = null) => {
        beginCompositionPass();
        if (view !== null) setMapView(JSON.stringify(view));
        beginCompositionPaint();
        paintedComposition([20, 100, width, height], [20, 100, width, height], [1000, 800]);
        await Promise.resolve();
    };
    return { workers, worker: workers[0], wrapper: nodes[1], draw, document, events, repaints: () => repaints };
}

test('direct presentation transfers canvas once and bounds resize work to one in flight', async t => {
    const b = browser(t);
    assert.equal(b.worker.url.href, 'https://example.test/api/hass/map-render-worker-ABCDEFG2.js');
    assert.equal(b.worker.messages[0].transfer.length, 1);
    b.worker.send('composition-ready', 'Gl', 8192);
    await b.draw();
    assert.equal(compositionStatus().inFlight, 1);
    for (const width of [610, 620, 630]) await b.draw(width);
    assert.equal(b.worker.messages.length, 2, 'resize churn cannot enqueue unbounded messages');
    b.worker.send('composition-drawn', 1, 600, 400);
    assert.deepEqual(b.worker.messages[2].message, [1, 'composition-size', 2, 630, 400]);
    b.worker.send('composition-drawn', 2, 630, 400);
    assert.equal(b.wrapper.style.display, 'block');
    assert.equal(compositionStatus().inFlight, 0);
    await b.draw(630);
    assert.equal(b.worker.messages.length, 3, 'unchanged dimensions do not redraw or transfer frames');
    assert.ok(b.worker.messages.slice(1).every(message => message.transfer === undefined));
});

test('logic-only click passes preserve presentation until actual paint', async t => {
    const b = browser(t);
    b.worker.send('composition-ready', 'Gl', 8192);
    await b.draw();
    b.worker.send('composition-drawn', 1, 600, 400);
    for (let event = 0; event < 4; event++) {
        beginCompositionPass();
        await Promise.resolve();
        assert.equal(b.wrapper.style.display, 'block', 'pointer logic must not hide the presented canvas');
    }
    beginCompositionPaint();
    paintedComposition([20, 100, 600, 400], [20, 100, 600, 400], [1000, 800]);
    await Promise.resolve();
    assert.equal(b.wrapper.style.display, 'block');
    assert.equal(b.worker.messages.length, 2, 'clicks do not redraw the stationary worker surface');
    beginCompositionPass();
    await Promise.resolve();
    assert.equal(b.wrapper.style.display, 'block');
    beginCompositionPaint();
    await Promise.resolve();
    assert.equal(b.wrapper.style.display, 'none', 'a painted frame without an opening removes it');
});

test('map transport coalesces view demand and transfers each route revision once', async t => {
    const b = browser(t, true);
    b.worker.send('composition-ready', 'Gl', 8192);
    setMapRoute(1, new Uint8Array([7, 8]));
    await b.draw(600, 400, { revision: 1, zoom: 10 });
    const frames = () => b.worker.messages.filter(m => m.message[1] === 'map-frame');
    assert.equal(frames().length, 1);
    assert.deepEqual(new Uint8Array(frames()[0].transfer[0]), new Uint8Array([7, 8]));
    for (const zoom of [11, 12, 13]) await b.draw(600, 400, { revision: 1, zoom });
    assert.equal(frames().length, 1, 'one submission stays in flight');
    b.worker.send('composition-drawn', 1, 600, 400);
    assert.equal(frames().length, 2);
    assert.equal(JSON.parse(frames()[1].message[5]).zoom, 13);
    assert.equal(frames()[1].message[6], null, 'camera updates do not resend the route');
    assert.equal(frames()[1].transfer, undefined);
    assert.equal(b.wrapper.style.display, 'block', 'in-flight camera work does not blink the surface');
    b.worker.send('composition-drawn', 2, 600, 400);
    setMapRoute(2, new Uint8Array([9]));
    await b.draw(600, 400, { revision: 2, zoom: 13 });
    assert.deepEqual(new Uint8Array(frames()[2].transfer[0]), new Uint8Array([9]));
});

test('readiness belongs to the current fully submitted view, not a draw acknowledgement', async t => {
    const b = browser(t, true);
    b.worker.send('composition-ready', 'Gl', 8192);
    setMapRoute(1, new Uint8Array([1]));
    await b.draw(600, 400, { revision: 1, zoom: 10 });
    b.worker.send('composition-drawn', 1, 600, 400);
    assert.equal(mapReadiness(), 'pending');
    b.worker.send('map-readiness', 1, 'ready');
    assert.equal(mapReadiness(), 'ready');
    await b.draw(600, 400, { revision: 1, zoom: 11 });
    assert.equal(mapReadiness(), 'pending');
    b.worker.send('map-readiness', 1, 'ready');
    assert.equal(mapReadiness(), 'pending');
    b.worker.send('composition-drawn', 2, 600, 400);
    assert.equal(mapReadiness(), 'pending');
    b.worker.send('map-readiness', 2, 'ready');
    assert.equal(mapReadiness(), 'ready');
    assert.throws(() => compositionMessage('map-readiness', 2, 'x'.repeat(1025)));
    assert.throws(() => compositionMessage('map-readiness', 0, 'ready'));
});

test('worker admission clocks yield without a Window or animation-frame API', async t => {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    let timeout = false,
        frame = false;
    const first = waitMilliseconds(1000).then(() => {
        timeout = true;
    });
    const second = nextMapFrame().then(() => {
        frame = true;
    });
    await Promise.resolve();
    assert.equal(timeout, false);
    assert.equal(frame, false);
    t.mock.timers.tick(16);
    await second;
    assert.equal(frame, true);
    assert.equal(timeout, false);
    t.mock.timers.tick(984);
    await first;
    assert.equal(timeout, true);
});

test('a painted frame without a map and page visibility hide the underlay', async t => {
    const b = browser(t);
    b.worker.send('composition-ready', 'Gl', 8192);
    await b.draw();
    b.worker.send('composition-drawn', 1, 600, 400);
    b.document.hidden = true;
    b.events.dispatchEvent(new Event('visibilitychange'));
    assert.equal(b.wrapper.style.display, 'none');
    b.document.hidden = false;
    b.events.dispatchEvent(new Event('visibilitychange'));
    await b.draw();
    assert.equal(b.wrapper.style.display, 'block');
    beginCompositionPass();
    beginCompositionPaint();
    await Promise.resolve();
    assert.equal(b.wrapper.style.display, 'none');
});

test('wrong backend fails explicitly, with no fallback', async t => {
    const b = browser(t);
    b.worker.send('composition-ready', 'BrowserWebGpu', 8192);
    assert.equal(compositionStatus().state, 'failed');
    assert.equal(b.worker.terminated, true);
    await b.draw();
    assert.equal(b.worker.messages.length, 1);
    assert.equal(b.wrapper.style.display, 'none');
});

test('uncaught worker errors stay local instead of escalating to application failure', t => {
    const b = browser(t);
    const event = new Event('error', { cancelable: true });
    Object.defineProperty(event, 'message', { value: 'worker adapter unavailable' });
    b.worker.onerror(event);
    assert.equal(event.defaultPrevented, true);
    assert.equal(compositionStatus().state, 'failed');
    assert.equal(compositionStatus().error, 'worker adapter unavailable');
    assert.equal(b.worker.terminated, true);
    assert.equal(b.wrapper.style.display, 'none');
});

test('device-limit violations release the worker', async t => {
    const b = browser(t);
    b.worker.send('composition-ready', 'Gl', 512);
    await b.draw();
    assert.equal(compositionStatus().state, 'failed');
    assert.match(compositionStatus().error, /device limits/);
    assert.equal(b.worker.terminated, true);
});

test('initialization timeout releases the worker and rejects late readiness', t => {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const b = browser(t);
    t.mock.timers.tick(60_000);
    assert.equal(compositionStatus().state, 'failed');
    assert.match(compositionStatus().error, /initialization timed out/);
    assert.equal(b.worker.terminated, true);
    b.worker.send('composition-ready', 'Gl', 8192);
    assert.equal(compositionStatus().state, 'failed');
    assert.equal(b.wrapper.style.display, 'none');
});

test('submission timeout releases the worker and rejects late publication', async t => {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const b = browser(t);
    b.worker.send('composition-ready', 'Gl', 8192);
    await b.draw();
    t.mock.timers.tick(60_000);
    assert.equal(compositionStatus().state, 'failed');
    assert.match(compositionStatus().error, /submission timed out/);
    assert.equal(compositionStatus().inFlight, 0);
    assert.equal(b.worker.terminated, true);
    b.worker.send('composition-drawn', 1, 600, 400);
    assert.equal(b.wrapper.style.display, 'none');
    assert.equal(compositionStatus().submitted, 0);
});

test('stale submission identities cannot publish a resized surface', async t => {
    const b = browser(t);
    b.worker.send('composition-ready', 'Gl', 8192);
    await b.draw();
    b.worker.send('composition-drawn', 2, 600, 400);
    assert.equal(compositionStatus().state, 'failed');
    assert.equal(b.wrapper.style.display, 'none');
    assert.equal(b.worker.terminated, true);
});

test('disposal releases the DOM surface and ignores late worker events', t => {
    const b = browser(t);
    disposeComposition();
    b.worker.send('composition-ready', 'Gl', 8192);
    assert.equal(b.wrapper.removed, true);
    assert.equal(b.worker.terminated, true);
    assert.deepEqual(compositionStatus(), { state: 'disposed' });
    assert.equal(window.garminMapExperiment, undefined);
});

test('render worker rejects early work and remains terminal after failure', async () => {
    const messages = [];
    const scope = { postMessage: message => messages.push(message) };
    installCompositionWorker(scope);
    await scope.onmessage({ data: compositionMessage('composition-size', 1, 100, 100) });
    assert.equal(messages[0][1], 'composition-failed');
    await scope.onmessage({ data: compositionMessage('composition-size', 2, 100, 100) });
    assert.equal(messages.length, 1);
});

test('render worker initializes the requested WASM and acknowledges actual submissions', async () => {
    const moduleUrl = `data:text/javascript,${encodeURIComponent(`
        export default async function(options) { if (options.module_or_path !== 'fixture.wasm') throw Error('wrong asset'); }
        export class CompositionRenderer {
            static async create(canvas, mode) {
                if (!canvas.transferredCanvas || mode !== 'worker-gl') throw Error('bad init');
                return new CompositionRenderer();
            }
            backend() { return 'Gl'; }
            maximum_size() { return 8192; }
            draw(w, h) { if (w !== 120 || h !== 80) throw Error('incorrect draw size'); }
            free() { throw Error('attempted to take ownership of Rust value while it was borrowed'); }
        }
    `)}`;
    const messages = [];
    const scope = { postMessage: message => messages.push(message) };
    installCompositionWorker(scope);
    const init = compositionMessage('composition-init', 'worker-gl', moduleUrl, 'fixture.wasm', {
        transferredCanvas: true,
    });
    await scope.onmessage({ data: init });
    assert.deepEqual(messages[0], [1, 'composition-ready', 'Gl', 8192]);
    await scope.onmessage({ data: compositionMessage('composition-size', 1, 120, 80) });
    assert.deepEqual(messages[1], [1, 'composition-drawn', 1, 120, 80]);
    await scope.onmessage({ data: compositionMessage('composition-size', 2, 121, 80) });
    assert.equal(messages[2][1], 'composition-failed', 'a failed draw must not acknowledge success');
    assert.equal(messages[2][2], 'Error: incorrect draw size', 'cleanup must not mask the original WASM failure');
    await scope.onmessage({ data: compositionMessage('composition-size', 3, 120, 80) });
    assert.equal(messages.length, 3, 'the failed renderer must never be used again');
});

test('worker reports bounded readiness transitions for asynchronous map draws', async t => {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const moduleUrl = `data:text/javascript,${encodeURIComponent(`
        export default async function() {}
        export class CompositionRenderer {
            static async create() { return new CompositionRenderer(); }
            backend() { return 'Gl'; }
            maximum_size() { return 8192; }
            enable_map() {}
            update_map() { this.draws = 0; }
            draw() { this.draws++; }
            readiness() { return this.draws > 1 ? 'ready' : 'pending'; }
        }
    `)}`;
    const messages = [];
    const scope = { postMessage: message => messages.push(message) };
    installCompositionWorker(scope);
    await scope.onmessage({
        data: compositionMessage('map-init', 'worker-gl', moduleUrl, 'fixture.wasm', {}, 'prepare.js'),
    });
    await scope.onmessage({ data: compositionMessage('map-frame', 1, 120, 80, '{}', new ArrayBuffer(0)) });
    assert.deepEqual(messages.at(-1), [1, 'map-readiness', 1, 'pending']);
    requestMapDraw(0);
    t.mock.timers.tick(16);
    assert.deepEqual(messages.at(-1), [1, 'map-readiness', 1, 'ready']);
    const count = messages.length;
    requestMapDraw(0);
    t.mock.timers.tick(16);
    assert.equal(messages.length, count, 'idle draws do not repeat readiness reports');
    await scope.onmessage({ data: compositionMessage('map-active', false) });
});
