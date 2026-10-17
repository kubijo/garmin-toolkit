// Browser API adapters. Rust owns records, identities, buffering, and RPC delivery.
// Trunk copies this adapter for workers and wasm-bindgen snippets. Share the callback
// across those module instances in each browser realm, without duplicate listeners.
const key = Symbol.for('garmin.application-log-bridge');
const bridge = (globalThis[key] ??= {
    sink: undefined,
    bootstrap: [],
    installed: false,
});

function deliver(payload) {
    if (bridge.sink) bridge.sink(payload);
    else if (typeof document === 'undefined' && typeof globalThis.postMessage === 'function') {
        globalThis.postMessage({ type: 'application-log', payload });
    } else if (bridge.bootstrap.length < 16) bridge.bootstrap.push(payload);
}

export function recordLog(level, component, message, source = typeof document === 'undefined' ? 'worker' : 'main') {
    deliver(
        JSON.stringify({
            kind: 'event',
            level,
            component,
            message: String(message).slice(0, 4096),
            source,
        }),
    );
}

export function receiveLog(data) {
    if (data?.type !== 'application-log') return false;
    if (typeof data.payload === 'string') deliver(data.payload);
    return true;
}

export function bindLogs(callback) {
    bridge.sink = callback;
    for (const payload of bridge.bootstrap.splice(0)) callback(payload);
}

export function postLog(payload) {
    globalThis.postMessage({ type: 'application-log', payload });
}

export function installLogs(scope = globalThis) {
    scope.addEventListener?.('error', event => recordLog('Error', 'javascript', event.message ?? 'JavaScript error'));
    scope.addEventListener?.('unhandledrejection', event => recordLog('Error', 'javascript', String(event.reason)));
}

if (!bridge.installed) {
    bridge.installed = true;
    installLogs();
}

export function downloadLogs(text) {
    const url = URL.createObjectURL(new Blob([text], { type: 'application/x-ndjson' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'garmin-logs.jsonl';
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
