import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';
import { fingerprintWeb, browserAssetPaths } from './fingerprint-web.mjs';

async function build(version, wasm = 'wasm fixture') {
    await mkdir('.tmp', { recursive: true });
    const root = await mkdtemp('.tmp/web-assets-test-');
    await mkdir(join(root, 'snippets', 'unchanged-directory-id'), { recursive: true });
    await writeFile(join(root, 'snippets', 'unchanged-directory-id', 'codec.js'), `export const version = ${version};`);
    await writeFile(join(root, 'worker-codec.js'), `export const version = ${version};`);
    for (const worker of ['map-worker', 'map-render-worker']) {
        await writeFile(join(root, `${worker}.js`), 'export { version } from "./worker-codec.js";');
    }
    await writeFile(
        join(root, 'garmin-hass-web-abcdef.js'),
        `
        export { version } from './snippets/unchanged-directory-id/codec.js';
        export default function init(module_or_path) {
            if (module_or_path === undefined) module_or_path = new URL('garmin-hass-web_bg.wasm', import.meta.url);
            return module_or_path;
        }
    `,
    );
    await writeFile(join(root, 'garmin-hass-web-abcdef_bg.wasm'), wasm);
    await writeFile(join(root, 'icon.svg'), '<svg/>');
    await writeFile(
        join(root, 'index.html'),
        '<html><head><link rel="icon" href="./icon.svg"><link rel="modulepreload" href="./garmin-hass-web-abcdef.js"></head><body></body></html>',
    );
    const names = await fingerprintWeb(root);
    const html = await readFile(join(root, 'index.html'), 'utf8');
    return { root, names, html, assets: browserAssetPaths(html) };
}

async function inventory(root, prefix = '') {
    const result = new Map();
    for (const entry of await readdir(join(root, prefix), { withFileTypes: true })) {
        const path = join(prefix, entry.name);
        if (entry.isDirectory()) {
            for (const [name, bytes] of await inventory(root, path)) result.set(name, bytes);
        } else result.set(path, await readFile(join(root, path)));
    }
    return result;
}

test('cached build A cannot supply stale dependencies to build B', async () => {
    const first = await build(1);
    const second = await build(2);
    for (const asset of ['module', 'map-worker', 'map-render-worker']) {
        assert.notEqual(
            first.assets[asset],
            second.assets[asset],
            `${asset} must include transitive dependency changes`,
        );
        const a = await import(pathToFileURL(join(first.root, first.assets[asset])).href);
        const b = await import(pathToFileURL(join(second.root, second.assets[asset])).href);
        assert.equal(a.version, 1);
        assert.equal(b.version, 2);
    }
    assert.equal(first.assets.wasm, second.assets.wasm, 'identical binary content keeps its URL');
    const oldCache = await inventory(first.root);
    for (const [path, bytes] of await inventory(second.root)) {
        if (path === 'index.html') continue;
        assert.match(path, /-[A-Z2-7]{8}\.[a-z]+$/, `unfingerprinted asset: ${path}`);
        if (oldCache.has(path))
            assert.deepEqual(oldCache.get(path), bytes, `immutable URL reused for different bytes: ${path}`);
    }
    assert.ok(!second.html.includes('href="./icon.svg"'));
    const app = await import(pathToFileURL(join(second.root, second.assets.module)).href);
    assert.equal(
        app.default().href,
        pathToFileURL(join(second.root, second.assets.wasm)).href,
        'implicit WASM initialization uses the file-loader URL',
    );
});

test('binary changes invalidate the dependent application module, while unchanged builds are reproducible', async () => {
    const first = await build(3, 'one');
    const repeated = await build(3, 'one');
    const changed = await build(3, 'two');
    assert.deepEqual(first.names, repeated.names);
    assert.notEqual(first.assets.wasm, changed.assets.wasm);
    assert.notEqual(first.assets.module, changed.assets.module);
    assert.equal(first.assets['map-worker'], changed.assets['map-worker']);
});

test('every emitted production asset is fingerprinted and every declared path exists', {
    skip: !process.env.GARMIN_TEST_WEB_ROOT && 'requires the emitted Trunk bundle',
}, async () => {
    const root = process.env.GARMIN_TEST_WEB_ROOT;
    const files = await inventory(root);
    for (const name of files.keys()) {
        if (name !== 'index.html')
            assert.match(name, /-[A-Z2-7]{8}\.[a-z]+$/, `unfingerprinted production asset: ${name}`);
    }
    const assets = browserAssetPaths(files.get('index.html').toString());
    assert.deepEqual(Object.keys(assets).sort(), ['map-render-worker', 'map-worker', 'module', 'wasm']);
    for (const path of Object.values(assets)) assert.ok(files.has(path), `missing declared asset: ${path}`);
});
