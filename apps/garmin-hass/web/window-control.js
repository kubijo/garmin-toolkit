// Window ownership stays with the existing host. Automation uses expiring handles.
const registries = new WeakMap();
function registry(browser) {
    let value = registries.get(browser);
    if (!value) {
        value = { next: 0, entries: new Map() };
        registries.set(browser, value);
    }
    return value;
}

export function registerWindow(key, specJson, child, browser = window) {
    if (!browser.garminAutomation) return;
    const state = registry(browser);
    state.entries.set(key, {
        key,
        spec: JSON.parse(specJson),
        child,
        api: undefined,
        id: key + ':' + ++state.next,
    });
    windowObservation(state.entries.get(key), browser);
}

export function unregisterWindow(key, browser = window) {
    const entry = registry(browser).entries.get(key);
    if (entry) windowObservation(entry, browser, true);
    registry(browser).entries.delete(key);
}

function refresh(browser) {
    const state = registry(browser);
    for (const [key, entry] of state.entries) {
        try {
            if (entry.child.closed) {
                unregisterWindow(key, browser);
                continue;
            }
            const api = entry.child.garminWindowControl;
            if (entry.api && entry.api !== api) {
                windowObservation(entry, browser, true);
                entry.id = key + ':' + ++state.next;
            }
            entry.api = api;
        } catch {
            // Navigation away from this origin revokes access permanently.
            unregisterWindow(key, browser);
        }
    }
    return state;
}

function selected(id, browser) {
    const entry = [...refresh(browser).entries.values()].find(entry => entry.id === id);
    if (!entry) throw Error('window is closed or its handle is stale');
    return entry;
}

function describe(id, kind, title, browser, ready) {
    return {
        id,
        kind,
        title,
        ready,
        focused: browser.document.hasFocus(),
        screenshots: true,
    };
}

export function routeWindowCommand(request, browser, invoke) {
    const root = request.window == null || request.window === 'root';
    if (request.operation === 'windows') {
        if (!root || request.argument != null) throw Error('windows takes no argument or child selector');
        return [
            describe('root', 'application', browser.document.title, browser, true),
            ...[...refresh(browser).entries.values()].map(entry =>
                describe(entry.id, entry.spec.kind, entry.spec.title, entry.child, !!entry.api),
            ),
        ];
    }
    const lifecycle = ['window.focus', 'window.close'].includes(request.operation);
    if (root) {
        if (lifecycle) throw Error('window focus/close requires an explicit child handle');
        return invoke(browser.garminAutomation);
    }
    const entry = selected(request.window, browser);
    if (lifecycle) {
        if (request.argument != null) throw Error('window focus/close takes no argument');
        if (request.operation === 'window.focus') entry.child.focus();
        else {
            entry.child.close();
            // Revoke immediately, even if the browser has not destroyed the surface yet.
            unregisterWindow(entry.key, browser);
        }
        return null;
    }
    if (!entry.api) throw Error('window has not initialized yet');
    return invoke(entry.api.automation);
}

export function installWindowDiagnostics(observe, browser = window) {
    registry(browser).observe = observe;
}

export function installWindowControl(capture, browser = window) {
    const observe = json => registry(browser).observe?.(json);
    browser.garminWindowControl = Object.freeze({
        automation: browser.garminAutomation,
        capture,
        observe,
        observeChild(child, json) {
            const entry = [...refresh(browser).entries.values()].find(entry => entry.child === child);
            if (!entry) return;
            const observation = JSON.parse(json);
            observation.window = entry.id;
            observe(JSON.stringify(observation));
        },
    });
}

export function publishObservation(json, browser = window) {
    if (browser.opener?.garminWindowControl?.observeChild)
        browser.opener.garminWindowControl.observeChild(browser, json);
    else browser.garminWindowControl?.observe?.(json);
}

function windowObservation(entry, browser, removed = false) {
    browser.garminWindowControl?.observe?.(
        JSON.stringify({
            kind: 'window',
            window: entry.id,
            removed,
            fields: {
                kind: entry.spec.kind,
                title: entry.spec.title,
                ready: String(!!entry.api),
                focused: String(!removed && entry.child.document.hasFocus()),
            },
        }),
    );
}

export function refreshDiagnostics(browser = window) {
    const entries = refresh(browser).entries.values();
    for (const entry of entries) windowObservation(entry, browser);
    publishObservation(
        JSON.stringify({
            kind: 'window',
            window: 'root',
            removed: false,
            fields: {
                kind: 'application',
                title: browser.document.title,
                focused: String(browser.document.hasFocus()),
            },
        }),
        browser,
    );
}

export async function captureWindow(id, milliseconds, browser = window) {
    if (!Number.isFinite(milliseconds) || milliseconds <= 0 || milliseconds > 5000)
        throw Error('invalid screenshot deadline');
    const entry = selected(id, browser);
    const api = entry.api;
    if (!api) throw Error('window has not initialized yet');
    const current = () => {
        if (selected(id, browser) !== entry || entry.api !== api)
            throw Error('window changed during screenshot capture');
    };
    let timer, poll;
    const lifetime = new Promise((_, reject) => {
        timer = setTimeout(() => reject(Error('screenshot deadline expired')), milliseconds);
        // A destroyed popup may never settle its promise. Check from the owning page.
        poll = setInterval(() => {
            try {
                current();
            } catch (error) {
                reject(error);
            }
        }, 25);
    });
    try {
        const result = await Promise.race([api.capture(milliseconds), lifetime]);
        current();
        if (
            !Array.isArray(result) ||
            result.length !== 2 ||
            typeof result[0] !== 'string' ||
            !ArrayBuffer.isView(result[1]) ||
            result[1].BYTES_PER_ELEMENT !== 1 ||
            result[1].byteLength > 8 * 1024 * 1024
        )
            throw Error('invalid or oversized window screenshot');
        // wasm-bindgen expects a Uint8Array from this page's JavaScript realm.
        return [result[0], new Uint8Array(result[1])];
    } finally {
        clearTimeout(timer);
        clearInterval(poll);
    }
}
