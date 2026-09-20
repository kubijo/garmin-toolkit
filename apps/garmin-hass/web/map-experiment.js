import { compositionMessage, decodeComposition } from './worker-codec.js';
import { applicationAssets } from './browser-assets.js';

export function experimentMode(enabled, search) {
    if (!enabled) return '';
    const values = new URLSearchParams(search).getAll('map-render-mode');
    if (values.length > 1) throw Error('map-render-mode must appear at most once');
    const mode = values[0] ?? 'worker-gl';
    if (!['main-gl', 'worker-gl', 'worker-webgpu'].includes(mode)) throw Error('unknown map-render-mode');
    return mode;
}

// Pure coordinate conversion shared by the host and tests. Projection is NEVER clipped/rebased.
export function placementStyle(projection, clip, screen, bounds) {
    const values = [...projection, ...clip, ...screen, bounds.left, bounds.top, bounds.width, bounds.height];
    if (
        !values.every(Number.isFinite) ||
        screen.some(v => v <= 0) ||
        projection[2] <= 0 ||
        projection[3] <= 0 ||
        clip[2] <= 0 ||
        clip[3] <= 0 ||
        bounds.width <= 0 ||
        bounds.height <= 0
    )
        throw Error('invalid composition placement');
    const sx = bounds.width / screen[0];
    const sy = bounds.height / screen[1];
    return {
        left: bounds.left + clip[0] * sx,
        top: bounds.top + clip[1] * sy,
        width: clip[2] * sx,
        height: clip[3] * sy,
        canvasLeft: (projection[0] - clip[0]) * sx,
        canvasTop: (projection[1] - clip[1]) * sy,
        canvasWidth: projection[2] * sx,
        canvasHeight: projection[3] * sy,
        backingWidth: Math.ceil(projection[2]),
        backingHeight: Math.ceil(projection[3]),
    };
}

let host;
let rendererConfiguration;
let pendingRendererDiagnostics = '';
let rendererDisposed = false;

const backendLabel = backend => ({ Gl: 'WebGL2', BrowserWebGpu: 'WebGPU' })[backend] ?? backend;

// One snapshot drives the automatic console table and the in-view diagnostic footer.
export function rendererSnapshot() {
    if (!rendererConfiguration) return null;
    const { mode, uiBackend, adapter, telemetry } = rendererConfiguration;
    const worker = mode.startsWith('worker-');
    return {
        mode,
        mapThread: worker ? 'worker' : 'main',
        mapBackend: worker ? (host?.backend ?? 'pending') : uiBackend,
        state: worker ? (host?.status ?? (rendererDisposed ? 'disposed' : 'initializing')) : 'ready',
        uiBackend,
        uiAdapter: adapter,
        uploadTelemetry: telemetry,
        error: worker ? (host?.error ?? '') : '',
    };
}

function reportRendererStatus() {
    const snapshot = rendererSnapshot();
    if (snapshot) {
        pendingRendererDiagnostics = rendererDiagnostics();
        console.table(snapshot);
    }
}

export function reportRendererStartup(mode, uiBackend, adapter, telemetry) {
    rendererDisposed = false;
    rendererConfiguration = {
        mode: mode || (uiBackend === 'Gl' ? 'main-gl' : 'main-webgpu'),
        uiBackend: backendLabel(uiBackend),
        adapter,
        telemetry,
    };
    reportRendererStatus();
}

// Only startup/state transitions cross into Rust; idle frames do not serialize diagnostics.
export function takeRendererDiagnostics() {
    const update = pendingRendererDiagnostics;
    pendingRendererDiagnostics = '';
    return update;
}

export function rendererDiagnostics() {
    const snapshot = rendererSnapshot();
    if (!snapshot) return '';
    return JSON.stringify({
        label: `${snapshot.mode} · ${snapshot.state}`,
        detail: Object.entries(snapshot)
            .map(([key, value]) => `${key}: ${value}`)
            .join('\n'),
    });
}

export function compositionFixture() {
    return new URLSearchParams(window.location.search).get('map-composition-proof') === '1';
}

export function startMap(canvas, mode, repaint) {
    startComposition(canvas, mode, repaint, true);
}

export function setMapRoute(revision, bytes) {
    if (host?.map) host.pendingRoute = { revision, buffer: bytes.slice().buffer };
}

export function setMapView(view) {
    if (host?.map) {
        if (host.pendingView !== null) {
            host.fail('simultaneous experimental maps are not supported');
            return;
        }
        host.pendingView = view;
    }
}

export function failMap(reason) {
    host?.fail(reason);
}
export function mapFailure() {
    return host?.status === 'failed' ? host.error : '';
}

export function startComposition(canvas, mode, repaint, map = false) {
    disposeComposition();
    rendererDisposed = false;
    const wrapper = document.createElement('div');
    wrapper.id = 'garmin-map-composition';
    Object.assign(wrapper.style, {
        position: 'fixed',
        overflow: 'hidden',
        pointerEvents: 'none',
        display: 'none',
        zIndex: '0',
    });
    const mapCanvas = document.createElement('canvas');
    Object.assign(mapCanvas.style, { position: 'absolute', margin: '0' });
    wrapper.append(mapCanvas);
    canvas.before(wrapper);
    const originalStyle = canvas.getAttribute('style');
    Object.assign(canvas.style, { position: 'relative', zIndex: '1' });
    const state = {
        canvas,
        mapCanvas,
        wrapper,
        originalStyle,
        repaint,
        mode,
        status: 'initializing',
        error: '',
        sequence: 0,
        sent: 0,
        bytes: 0,
        placement: null,
        flight: null,
        drawn: null,
        flushQueued: false,
        maximumSize: 0,
        map,
        view: null,
        route: null,
        pendingView: null,
        pendingRoute: null,
        routeRevision: 0,
        active: false,
    };
    host = state;
    state.visibility = () => {
        if (document.hidden) {
            state.wrapper.style.display = 'none';
            setActive(state, false);
        } else state.repaint();
    };
    document.addEventListener('visibilitychange', state.visibility);
    const fail = error => {
        if (host !== state || state.status === 'failed') return;
        state.status = 'failed';
        state.error = String(error).slice(0, 1024);
        reportRendererStatus();
        state.wrapper.style.display = 'none';
        state.worker?.terminate();
        state.flight = null;
        clearTimeout(state.timeout);
        state.repaint();
        console.error('Map composition experiment failed:', state.error);
    };
    state.fail = fail;
    try {
        const { moduleUrl, wasmUrl, preparationUrl, renderUrl } = applicationAssets();
        const offscreen = mapCanvas.transferControlToOffscreen();
        const worker = new Worker(new URL(renderUrl), { type: 'module' });
        state.worker = worker;
        worker.onerror = event => {
            // This worker has its own visible failure state. Do not escalate a handled map
            // failure to the initializer's whole-application window error handler.
            event.preventDefault();
            fail(event.message || 'render worker error');
        };
        worker.onmessageerror = () => fail('unreadable render worker message');
        worker.onmessage = ({ data }) => {
            if (host !== state || state.status === 'failed') return;
            try {
                const message = decodeComposition(data);
                if (message.type === 'composition-ready' && state.status === 'initializing') {
                    const expected = mode === 'worker-gl' ? 'Gl' : 'BrowserWebGpu';
                    if (message.backend !== expected)
                        throw Error('worker backend does not match the requested backend');
                    clearTimeout(state.timeout);
                    state.status = 'ready';
                    state.backend = backendLabel(message.backend);
                    reportRendererStatus();
                    state.maximumSize = message.maximumSize;
                    state.repaint();
                    flush(state);
                } else if (
                    message.type === 'composition-drawn' &&
                    state.flight?.id === message.id &&
                    state.flight.width === message.width &&
                    state.flight.height === message.height
                ) {
                    clearTimeout(state.timeout);
                    state.drawn = state.flight;
                    if (state.map) state.routeRevision = state.flight.revision;
                    state.flight = null;
                    flush(state);
                    state.repaint();
                } else if (message.type === 'composition-failed') {
                    fail(message.reason);
                } else {
                    throw Error('unexpected or stale composition response');
                }
            } catch (error) {
                fail(error);
            }
        };
        state.timeout = setTimeout(() => fail('render worker initialization timed out'), 10000);
        const initialization = map
            ? compositionMessage('map-init', mode, moduleUrl, wasmUrl, offscreen, preparationUrl)
            : compositionMessage('composition-init', mode, moduleUrl, wasmUrl, offscreen);
        worker.postMessage(initialization, [offscreen]);
    } catch (error) {
        fail(error);
    }
    window.garminMapExperiment = Object.freeze({ status: compositionStatus, dispose: disposeComposition });
}

export function beginCompositionPass() {
    if (host) host.pendingView = null;
}

export function beginCompositionPaint() {
    if (!host) return;
    const state = host;
    state.placement = null;
    state.view = state.pendingView;
    if (state.pendingRoute && state.view !== null && state.pendingRoute.revision === JSON.parse(state.view).revision) {
        state.route = state.pendingRoute;
        state.pendingRoute = null;
    }
    if (!state.flushQueued) {
        state.flushQueued = true;
        queueMicrotask(() => {
            state.flushQueued = false;
            if (host === state) flush(state);
        });
    }
}

export function paintedComposition(projection, clip, screen) {
    if (host)
        host.placement = { projection: Array.from(projection), clip: Array.from(clip), screen: Array.from(screen) };
}

function flush(state) {
    if (state.status !== 'ready' || !state.placement || document.hidden) {
        state.wrapper.style.display = 'none';
        setActive(state, false);
        return;
    }
    try {
        const { projection, clip, screen } = state.placement;
        const style = placementStyle(projection, clip, screen, state.canvas.getBoundingClientRect());
        const width = style.backingWidth,
            height = style.backingHeight;
        if (width > state.maximumSize || height > state.maximumSize)
            throw Error('composition surface exceeds device limits');
        Object.assign(state.wrapper.style, {
            left: `${style.left}px`,
            top: `${style.top}px`,
            width: `${style.width}px`,
            height: `${style.height}px`,
        });
        Object.assign(state.mapCanvas.style, {
            left: `${style.canvasLeft}px`,
            top: `${style.canvasTop}px`,
            width: `${style.canvasWidth}px`,
            height: `${style.canvasHeight}px`,
        });
        const matches = state.drawn?.width === width && state.drawn?.height === height;
        state.wrapper.style.display = matches && (state.map || !state.flight) ? 'block' : 'none';
        setActive(state, true);
        const viewChanged = state.map && state.view !== null && state.view !== state.drawn?.view;
        if ((!matches || viewChanged) && !state.flight) {
            if (state.map && state.view === null) return;
            if (state.sequence === 0xffffffff) throw Error('composition sequence exhausted');
            const id = ++state.sequence;
            const revision = state.map ? JSON.parse(state.view).revision : 0;
            let route = null;
            if (state.map && revision !== state.routeRevision) {
                if (state.route?.revision !== revision) throw Error('map route revision is missing');
                route = state.route.buffer;
            }
            const message = state.map
                ? compositionMessage('map-frame', id, width, height, state.view, route)
                : compositionMessage('composition-size', id, width, height);
            state.flight = { id, width, height, view: state.view, revision };
            state.sent += 1;
            state.bytes += JSON.stringify(message).length;
            state.timeout = setTimeout(() => state.fail('render worker submission timed out'), 10000);
            if (route === null) state.worker.postMessage(message);
            else state.worker.postMessage(message, [route]);
            if (route !== null) state.route = null;
        }
    } catch (error) {
        state.fail(error);
    }
}

function setActive(state, active) {
    if (!state.map || state.status !== 'ready' || state.active === active) return;
    state.active = active;
    state.worker.postMessage(compositionMessage('map-active', active));
}

export function compositionStatus() {
    if (!host) return { state: 'disposed' };
    return {
        state: host.status,
        mode: host.mode,
        error: host.error,
        sent: host.sent,
        controlBytes: host.bytes,
        inFlight: Number(host.flight !== null),
        submitted: host.drawn?.id ?? 0,
    };
}

export function compositionStatusText() {
    const status = compositionStatus();
    return status.error ? `${status.state}: ${status.error}` : `${status.state} · submissions ${status.submitted ?? 0}`;
}

export function disposeComposition() {
    if (!host) return;
    const state = host;
    host = undefined;
    rendererDisposed = true;
    reportRendererStatus();
    clearTimeout(state.timeout);
    document.removeEventListener('visibilitychange', state.visibility);
    state.worker?.terminate();
    state.wrapper.remove();
    if (state.originalStyle === null) state.canvas.removeAttribute('style');
    else state.canvas.setAttribute('style', state.originalStyle);
    delete window.garminMapExperiment;
}
