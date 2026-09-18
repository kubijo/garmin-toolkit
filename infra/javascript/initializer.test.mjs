import assert from 'node:assert/strict';
import test from 'node:test';
import initialize from '../../apps/garmin-hass/web/initializer.js';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

function loader(t) {
    const elements = new Map();
    const frames = new Map();
    let nextFrame = 0;
    const document = {
        querySelector(selector) {
            const element = {
                isConnected: true,
                hidden: false,
                dataset: {},
                textContent: '',
                style: { setProperty() {} },
                setAttribute() {},
                addEventListener() {},
            };
            elements.set(selector, element);
            return element;
        },
    };
    const events = new EventTarget();
    // In a browser, Window is the global object. web_sys::window checks that identity.
    class Window {
        static [Symbol.hasInstance](value) {
            return value === globalThis;
        }
    }
    const window = globalThis;
    const globals = {
        document,
        window,
        Window,
        addEventListener: events.addEventListener.bind(events),
        dispatchEvent: events.dispatchEvent.bind(events),
        requestAnimationFrame: callback => {
            frames.set(++nextFrame, callback);
            return nextFrame;
        },
        cancelAnimationFrame: id => frames.delete(id),
    };
    for (const [key, value] of Object.entries(globals)) {
        const original = Object.getOwnPropertyDescriptor(globalThis, key);
        Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
        t.after(() => (original ? Object.defineProperty(globalThis, key, original) : delete globalThis[key]));
    }
    t.mock.method(console, 'error', () => {});
    const hooks = initialize();
    return {
        hooks,
        frames,
        get: name => elements.get(`#loading${name ? `-${name}` : ''}`),
        send: (name, detail) => window.dispatchEvent(new CustomEvent(`garmin-toolkit-${name}`, { detail })),
        flush: () => {
            const callbacks = [...frames.values()];
            frames.clear();
            callbacks.forEach(fn => fn());
        },
    };
}

test('failure rejects later start/progress and ready notifications', t => {
    const instance = loader(t);
    instance.hooks.onFailure(Error('original download failure'));
    instance.hooks.onStart();
    instance.hooks.onProgress({ current: 100, total: 100 });
    instance.send('ready');
    instance.flush();
    assert.equal(instance.frames.size, 0);
    assert.equal(instance.get('detail').textContent, 'original download failure');
    assert.equal(instance.get('message').textContent, 'Garmin Toolkit could not start.');
    assert.equal(instance.get('').hidden, false);
});

test('failure event cancels pending progress and survives an already dispatched callback and trailing trap', t => {
    const instance = loader(t);
    instance.hooks.onProgress({ current: 100, total: 100 });
    const staleCallback = [...instance.frames.values()][0];
    instance.send('failure', 'wgpu validation failed');
    assert.equal(instance.frames.size, 0);
    staleCallback();
    instance.hooks.onFailure(new WebAssembly.RuntimeError('unreachable'));
    assert.equal(instance.get('detail').textContent, 'wgpu validation failed');
});

test('ready hides loading, ignores progress, but still surfaces a later runtime panic', t => {
    const instance = loader(t);
    instance.hooks.onProgress({ current: 100, total: 100 });
    instance.flush();
    assert.equal(instance.get('detail').textContent, 'Download complete');
    instance.send('ready');
    instance.hooks.onProgress({ current: 50, total: 100 });
    assert.equal(instance.frames.size, 0);
    assert.equal(instance.get('').hidden, true);
    instance.send('failure', 'runtime panic');
    assert.equal(instance.get('').hidden, false);
    assert.equal(instance.get('detail').textContent, 'runtime panic');
});

test('emitted WASM reports the original failure through the Rust CustomEvent bridge', {
    skip: !process.env.GARMIN_TEST_WEB_ROOT && 'requires emitted WASM; mandatory in the web package build',
}, async t => {
    const root = process.env.GARMIN_TEST_WEB_ROOT;
    const html = await readFile(join(root, 'index.html'), 'utf8');
    const js = html.match(/garmin-hass-web-[a-z0-9]+\.js/)?.[0];
    const wasm = html.match(/garmin-hass-web-[a-z0-9]+_bg\.wasm/)?.[0];
    assert.ok(js && wasm);
    // Initialize as a worker (no Window) so this test does not start a renderer.
    const module = await import(pathToFileURL(join(root, js)).href);
    await module.default({ module_or_path: await readFile(join(root, wasm)) });
    const instance = loader(t);
    instance.hooks.onProgress({ current: 100, total: 100 });
    module.show_browser_failure('original Rust failure');
    instance.hooks.onFailure(new WebAssembly.RuntimeError('unreachable'));
    instance.flush();
    assert.equal(instance.get('detail').textContent, 'original Rust failure');
    assert.equal(instance.get('').dataset.state, 'failure');
    assert.equal(instance.frames.size, 0);
});
