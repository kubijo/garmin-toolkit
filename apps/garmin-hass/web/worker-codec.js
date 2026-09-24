function integer(value, maximum) {
    return Number.isInteger(value) && value >= 0 && value <= maximum;
}

function taskKind(kind) {
    return kind === 'tile' || kind === 'labels' || kind === 'route';
}

function buffer(value) {
    return value instanceof ArrayBuffer;
}

export function decode(value) {
    if (!Array.isArray(value) || !integer(value[0], 255)) throw Error('malformed worker envelope');
    const [version, type, id, kind, ...payload] = value;
    if (type === 'init' && value.length === 4 && typeof id === 'string' && id && typeof kind === 'string' && kind) {
        return { version, type, moduleUrl: id, wasmUrl: kind };
    }
    if (type === 'ready' && value.length === 2) return { version, type };
    if (type === 'fatal' && value.length === 3 && typeof id === 'string') return { version, type, reason: id };
    if (!integer(id, 0xffffffff) || !taskKind(kind)) throw Error('malformed worker task identity');
    if (type === 'task-error' && payload.length === 1 && typeof payload[0] === 'string') {
        return { version, type, id, kind, reason: payload[0] };
    }
    if (type === 'result' && payload.length === (kind === 'tile' ? 4 : 1) && payload.every(buffer)) {
        return { version, type, id, kind, buffers: payload };
    }
    if (type === 'task') {
        if (kind === 'tile' && payload.length === 4) {
            const [zoom, x, y, darkMode] = payload;
            if (
                integer(zoom, 14) &&
                integer(x, 2 ** zoom - 1) &&
                integer(y, 2 ** zoom - 1) &&
                typeof darkMode === 'boolean'
            ) {
                return { version, type, id, kind, zoom, x, y, darkMode };
            }
        } else if (kind !== 'tile' && payload.length === 1 && buffer(payload[0])) {
            return { version, type, id, kind, buffers: payload };
        }
    }
    throw Error('malformed worker payload');
}

function checked(message) {
    decode(message);
    return message;
}

export const initMessage = (version, moduleUrl, wasmUrl) => checked([version, 'init', moduleUrl, wasmUrl]);
export const tileTask = (version, id, zoom, x, y, darkMode) =>
    checked([version, 'task', id, 'tile', zoom, x, y, darkMode]);
export const dataTask = (version, id, kind, bytes) => checked([version, 'task', id, kind, bytes]);
export const ready = version => checked([version, 'ready']);
export const fatal = (version, reason) => checked([version, 'fatal', reason]);
export const result = (version, id, kind, buffers) => checked([version, 'result', id, kind, ...buffers]);
export const taskError = (version, id, kind, reason) => checked([version, 'task-error', id, kind, reason]);

// Composition gate messages deliberately share this codec between browser host and worker.
// Frame pixels cross this boundary only for an explicit bounded screenshot request.
const MAX_COMPOSITION_PIXELS = 16 * 1024 * 1024;

export function decodeComposition(value) {
    if (!Array.isArray(value) || value[0] !== 1) throw Error('invalid composition protocol');
    const [, type, ...payload] = value;
    if (type === 'composition-capture' && payload.length === 4) {
        const [id, viewId, width, height] = payload;
        decodeComposition([1, 'composition-size', viewId, width, height]);
        if (!integer(id, 0xffffffff) || id === 0 || width * height > 8 * 1024 * 1024)
            throw Error('invalid composition capture');
        return { type, id, viewId, width, height };
    }
    if (type === 'composition-captured' && payload.length === 2) {
        const [id, blob] = payload;
        if (
            integer(id, 0xffffffff) &&
            id > 0 &&
            blob instanceof Blob &&
            blob.type === 'image/png' &&
            blob.size > 0 &&
            blob.size <= 8 * 1024 * 1024
        )
            return { type, id, blob };
        throw Error('invalid composition capture reply');
    }
    if (type === 'composition-capture-failed' && payload.length === 2) {
        const [id, reason] = payload;
        if (integer(id, 0xffffffff) && id > 0 && typeof reason === 'string' && reason.length <= 1024)
            return { type, id, reason };
        throw Error('invalid composition capture failure');
    }
    if (type === 'map-readiness' && payload.length === 2) {
        const [id, state] = payload;
        if (
            integer(id, 0xffffffff) &&
            id > 0 &&
            typeof state === 'string' &&
            state.length <= 1024 &&
            (['pending', 'ready'].includes(state) || state.startsWith('failed:'))
        )
            return { type, id, state };
        throw Error('invalid map readiness');
    }
    if (['composition-init', 'map-init'].includes(type) && payload.length === (type === 'map-init' ? 5 : 4)) {
        const [mode, moduleUrl, wasmUrl, canvas, preparationUrl] = payload;
        if (
            ['worker-gl', 'worker-webgpu'].includes(mode) &&
            typeof moduleUrl === 'string' &&
            moduleUrl &&
            typeof wasmUrl === 'string' &&
            wasmUrl &&
            canvas !== null &&
            typeof canvas === 'object' &&
            (type !== 'map-init' || (typeof preparationUrl === 'string' && preparationUrl))
        )
            return { type, mode, moduleUrl, wasmUrl, canvas, preparationUrl };
    }
    if (type === 'map-active' && payload.length === 1 && typeof payload[0] === 'boolean') {
        return { type, active: payload[0] };
    }
    if (type === 'map-frame' && payload.length === 5) {
        const [id, width, height, view, route] = payload;
        decodeComposition([1, 'composition-size', id, width, height]);
        if (
            typeof view !== 'string' ||
            new TextEncoder().encode(view).length > 1536 ||
            new TextEncoder().encode(JSON.stringify([1, type, id, width, height, view, null])).length > 2048 ||
            (route !== null && (!(route instanceof ArrayBuffer) || route.byteLength > 32 * 1024 * 1024))
        )
            throw Error('invalid remote map payload');
        return { type, id, width, height, view, route };
    }
    if (type === 'composition-ready' && payload.length === 2) {
        const [backend, maximumSize] = payload;
        if (['Gl', 'BrowserWebGpu'].includes(backend) && integer(maximumSize, 32768) && maximumSize > 0) {
            return { type, backend, maximumSize };
        }
    }
    if (['composition-size', 'composition-drawn'].includes(type) && payload.length === 3) {
        const [id, width, height] = payload;
        if (
            integer(id, 0xffffffff) &&
            id > 0 &&
            [width, height].every(v => integer(v, 32768) && v > 0) &&
            width * height <= MAX_COMPOSITION_PIXELS
        ) {
            return { type, id, width, height };
        }
    }
    if (
        type === 'composition-failed' &&
        payload.length === 1 &&
        typeof payload[0] === 'string' &&
        payload[0].length <= 1024
    ) {
        return { type, reason: payload[0] };
    }
    throw Error('malformed composition message');
}

export function compositionMessage(type, ...payload) {
    const message = [1, type, ...payload];
    decodeComposition(message);
    return message;
}

// Called by worker-local wgpu device-loss/error callbacks. No DOM or browser-host dependency.
export function compositionWorkerFailure(reason) {
    globalThis.postMessage(compositionMessage('composition-failed', String(reason).slice(0, 1024)));
}

// The render-worker controller owns scheduling; Rust never captures JS handles in egui callbacks.
const mapDrawRequest = Symbol.for('garmin.map.draw-request');
// wasm-bindgen emits a second URL for this module. Both copies share this worker-local hook.
export function installMapDrawRequest(callback) {
    globalThis[mapDrawRequest] = callback;
}
export function requestMapDraw(milliseconds) {
    globalThis[mapDrawRequest]?.(milliseconds);
}
