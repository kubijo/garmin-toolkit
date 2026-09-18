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
