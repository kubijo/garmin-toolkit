import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

test('a failed module script surfaces a retryable error before WASM starts', async () => {
    const html = await readFile('apps/garmin-hass/web/index.html', 'utf8');
    const bootstrap = html.match(/<script nonce="GARMIN_TOOLKIT_CSP_NONCE">([\s\S]*?)<\/script>/)?.[1];
    assert.ok(bootstrap);

    const listeners = new Map();
    const elements = new Map();
    let reloads = 0;
    for (const id of ['loading', 'loading-message', 'loading-detail', 'loading-trace', 'loading-retry']) {
        elements.set(id, {
            hidden: id === 'loading-retry',
            dataset: {},
            textContent: '',
            attributes: {},
            setAttribute(name, value) {
                this.attributes[name] = value;
            },
            addEventListener(name, callback) {
                listeners.set(`${id}.${name}`, callback);
            },
        });
    }
    const document = {
        readyState: 'loading',
        getElementById: id => elements.get(id),
        addEventListener: (name, callback) => listeners.set(name, callback),
    };
    runInNewContext(bootstrap, {
        document,
        location: { reload: () => reloads++ },
        addEventListener: (name, callback) => listeners.set(name, callback),
    });

    listeners.get('error')({ target: { tagName: 'LINK', rel: 'icon' } });
    assert.equal(listeners.has('DOMContentLoaded'), false);
    listeners.get('error')({ target: { tagName: 'LINK', rel: 'modulepreload' } });
    assert.equal(listeners.has('DOMContentLoaded'), false);
    listeners.get('error')({ target: { tagName: 'SCRIPT', type: 'module' } });
    assert.equal(elements.get('loading').dataset.state, undefined);
    listeners.get('DOMContentLoaded')();

    assert.equal(elements.get('loading').dataset.state, 'failure');
    assert.equal(elements.get('loading').attributes.role, 'alert');
    assert.equal(elements.get('loading-retry').hidden, false);
    assert.match(elements.get('loading-detail').textContent, /could not load/);
    listeners.get('loading-retry.click')();
    assert.equal(reloads, 1);
});
