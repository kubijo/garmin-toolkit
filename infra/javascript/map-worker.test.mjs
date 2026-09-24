import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import { installMapWorker } from '../../apps/garmin-hass/web/map-worker.js';
import { recordLog } from '../../apps/garmin-hass/web/logging.js';
import * as codec from '../../apps/garmin-hass/web/worker-codec.js';
import { browserAssetPaths } from './fingerprint-web.mjs';

function worker(install = installMapWorker) {
    const messages = [];
    const logs = [];
    const self = {
        location: { href: 'https://example.test/api/hass/map-worker.js' },
        postMessage(message, transfer = []) {
            const copy = structuredClone(message, { transfer });
            if (copy?.type === 'application-log') logs.push(JSON.parse(copy.payload));
            else messages.push(copy);
        },
    };
    install(self);
    return { messages, logs, postMessage: self.postMessage, send: data => self.onmessage({ data }) };
}

test('passes the exact hashed WASM URL before accepting work', async () => {
    const wasmUrl = 'https://example.test/api/hass/garmin-hass-web-abcdef_bg.wasm';
    const moduleUrl = `data:text/javascript,${encodeURIComponent(`
        export default async function(options) {
            if (options?.module_or_path !== ${JSON.stringify(wasmUrl)}) throw Error('wrong WASM URL');
        }
        export function browser_worker_protocol_version() { return 2; }
        export function prepare_map_route(bytes) { return bytes; }
    `)}`;
    const instance = worker();
    await instance.send([2, 'init', moduleUrl, wasmUrl]);
    assert.deepEqual(Array.from(instance.messages[0]), [2, 'ready']);
    await instance.send([2, 'task', 1, 'route', new Uint8Array([4, 5]).buffer]);
    assert.deepEqual(Array.from(instance.messages[1]).slice(0, 4), [2, 'result', 1, 'route']);
    assert.deepEqual(new Uint8Array(instance.messages[1][4]), new Uint8Array([4, 5]));
});

test('missing WASM URL fails initialization explicitly', async () => {
    const instance = worker();
    await instance.send([2, 'init', 'https://example.test/app.js']);
    assert.equal(instance.messages[0][1], 'fatal');
    assert.match(instance.messages[0][2], /malformed worker/);
});

test('initializes emitted WASM and transfers empty and nonempty tiles', {
    skip: !process.env.GARMIN_TEST_WEB_ROOT && 'set GARMIN_TEST_WEB_ROOT to the built web asset directory',
}, async () => {
    const root = process.env.GARMIN_TEST_WEB_ROOT;
    const html = await readFile(join(root, 'index.html'), 'utf8');
    const assets = browserAssetPaths(html);
    const { installMapWorker: emittedWorker } = await import(pathToFileURL(join(root, assets['map-worker'])).href);
    const moduleName = assets.module;
    const wasmName = assets.wasm;
    assert.ok(moduleName, 'built HTML names a hashed JS module');
    assert.ok(wasmName, 'built HTML names a hashed WASM module');
    const moduleUrl = pathToFileURL(join(root, moduleName)).href;
    const wasmUrl = `https://example.test/api/hass/${wasmName}`;
    let tileUrl = 'https://example.test/api/hass/map/tiles/14/0/0.pbf';
    const wasm = await readFile(join(root, wasmName));
    const fixture = Buffer.from(
        (await readFile(new URL('fixtures/place.pbf.hex', import.meta.url), 'utf8')).trim(),
        'hex',
    );
    let tile = new Uint8Array();
    const originalFetch = globalThis.fetch;
    const originalPostMessage = globalThis.postMessage;
    const instance = worker(emittedWorker);
    // wasm-bindgen calls the real worker global; JS protocol replies use the
    // injected scope. Both must reach the same transport, including log envelopes.
    globalThis.postMessage = instance.postMessage;
    const requests = [];
    globalThis.fetch = async url => {
        requests.push(String(url));
        if (String(url) === tileUrl) return new Response(tile);
        assert.equal(String(url), wasmUrl, 'worker must fetch the emitted asset, never the unhashed default');
        return new Response(wasm, { headers: { 'Content-Type': 'application/wasm' } });
    };
    try {
        await instance.send([2, 'init', moduleUrl, wasmUrl]);
        assert.deepEqual(Array.from(instance.messages[0]), [2, 'ready']);
        assert.deepEqual(requests, [wasmUrl]);
        recordLog('Info', 'map-worker-test', 'emitted WASM logging probe', 'preparation-worker');
        const records = instance.logs.filter(log => log.record?.component === 'map-worker-test');
        assert.equal(records.length, 1, 'Rust must relay the worker event exactly once');
        assert.equal(records[0].kind, 'record');
        assert.equal(records[0].record.message, 'emitted WASM logging probe');
        assert.equal(records[0].record.source, 'preparation-worker');
        assert.ok(records[0].record.session);
        assert.ok(Number.isSafeInteger(records[0].record.source_sequence));
        await instance.send(codec.tileTask(2, 1, 14, 0, 0, true));
        assert.deepEqual(requests, [wasmUrl, tileUrl]);
        assert.deepEqual(instance.messages[1].slice(0, 4), [2, 'result', 1, 'tile']);
        const parts = instance.messages[1].slice(4);
        assert.equal(parts.length, 4);
        assert.ok(parts.every(part => part instanceof ArrayBuffer && part.byteLength === 0));
        tile = fixture;
        await instance.send(codec.tileTask(2, 2, 14, 0, 0, true));
        const nonempty = codec.decode(instance.messages[2]);
        assert.equal(nonempty.type, 'result');
        assert.ok(nonempty.buffers.every(part => part.byteLength > 0));
        assert.equal(new TextDecoder().decode(nonempty.buffers[3]), 'Helsinki 🚲');
        const vertexCount = nonempty.buffers[0].byteLength / 12;
        const indices = new Uint32Array(nonempty.buffers[1]);
        assert.ok(indices.length > 0 && indices.length % 3 === 0);
        assert.ok(indices.every(index => index < vertexCount));
        tile = new Uint8Array(2 * 1024 * 1024 + 1);
        await instance.send(codec.tileTask(2, 3, 14, 0, 0, true));
        assert.equal(codec.decode(instance.messages[3]).type, 'task-error');
        assert.match(codec.decode(instance.messages[3]).reason, /2 MiB/);
        tile = Buffer.concat(Array(65).fill(fixture));
        await instance.send(codec.tileTask(2, 4, 14, 0, 0, true));
        assert.equal(codec.decode(instance.messages[4]).type, 'task-error');
        assert.match(codec.decode(instance.messages[4]).reason, /layer or feature limit/);
        tile = Buffer.from(
            (await readFile(new URL('fixtures/extreme-polygon.pbf.hex', import.meta.url), 'utf8')).trim(),
            'hex',
        );
        await instance.send(codec.tileTask(2, 5, 14, 0, 0, true));
        assert.equal(codec.decode(instance.messages[5]).type, 'task-error');
        assert.match(codec.decode(instance.messages[5]).reason, /coordinates exceeded/);
        tile = fixture;
        await instance.send(codec.tileTask(2, 6, 14, 0, 0, true));
        assert.equal(
            codec.decode(instance.messages[6]).type,
            'result',
            'rejected geometry must not trap or kill the worker',
        );
        let id = 7;
        for (const [fixture, zoom, x, y] of [
            ['world-z0', 0, 0, 0],
            ['europe-africa-z2', 2, 1, 1],
            ['europe-africa-z3', 3, 3, 2],
            ['asia-z3', 3, 5, 2],
            ['london-z7', 7, 62, 44],
            ['london-z8', 8, 127, 85],
            ['london-z9', 9, 257, 169],
            ['london-z11', 11, 1023, 680],
            ['london-z10', 10, 511, 340],
        ]) {
            tile = await readFile(new URL(`fixtures/${fixture}.pbf`, import.meta.url));
            tileUrl = `https://example.test/api/hass/map/tiles/${zoom}/${x}/${y}.pbf`;
            for (const dark of [false, true]) {
                await instance.send(codec.tileTask(2, id++, zoom, x, y, dark));
                const response = codec.decode(instance.messages.at(-1));
                assert.equal(response.type, 'result', `${zoom}/${x}/${y}: ${response.reason}`);
                assert.ok(response.buffers.every(part => part.byteLength > 0));
                const vertices = response.buffers[0].byteLength / 12;
                const indices = new Uint32Array(response.buffers[1]);
                assert.equal(indices.length % 3, 0);
                assert.ok(indices.every(index => index < vertices));
            }
        }
    } finally {
        globalThis.fetch = originalFetch;
        if (originalPostMessage === undefined) delete globalThis.postMessage;
        else globalThis.postMessage = originalPostMessage;
    }
});

test('one codec validates every envelope and rejects malformed identity and buffers', () => {
    assert.deepEqual(codec.decode(codec.tileTask(2, 7, 4, 3, 8, true)), {
        version: 2,
        type: 'task',
        id: 7,
        kind: 'tile',
        zoom: 4,
        x: 3,
        y: 8,
        darkMode: true,
    });
    const bytes = new Uint8Array([4, 5]).buffer;
    assert.equal(codec.decode(codec.dataTask(2, 8, 'route', bytes)).buffers[0], bytes);
    assert.equal(codec.decode(codec.result(2, 8, 'route', [bytes])).type, 'result');
    assert.equal(codec.decode(codec.taskError(2, 8, 'route', 'failure')).reason, 'failure');
    for (const invalid of [
        [2, 'task', -1, 'route', bytes],
        [2, 'task', 1.5, 'route', bytes],
        [2, 'task', 1, 'tile', 4, 16, 0, true],
        [2, 'task', 1, 'unknown', bytes],
        [2, 'result', 1, 'tile', bytes],
        [2, 'result', 1, 'route', new Uint8Array(bytes)],
        [2, 'ready', 'extra'],
        [NaN, 'ready'],
        [256, 'ready'],
    ])
        assert.throws(() => codec.decode(invalid), /malformed/);
});
