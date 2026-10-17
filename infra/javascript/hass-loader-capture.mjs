// Capture the source HTML's pre-WASM loader when app control cannot start.
// Run against a disposable demo whose web root is apps/garmin-hass/web.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const [url = 'http://127.0.0.1:8099/', modulePath = '/initializer.js', widthArg = '1280', heightArg = '720'] =
    process.argv.slice(2);
const target = new URL(url);
const controlTarget = new URL(process.env.HASS_CONTROL_URL ?? url);
const packaged = modulePath === '@packaged';
assert.equal(target.protocol, 'http:', 'the demo uses loopback HTTP');
assert.ok(['localhost', '127.0.0.1', '[::1]'].includes(target.hostname), 'the demo must be on loopback');
assert.equal(controlTarget.protocol, 'http:', 'control uses loopback HTTP');
assert.ok(['localhost', '127.0.0.1', '[::1]'].includes(controlTarget.hostname), 'control must stay on loopback');
assert.ok(packaged || (modulePath.startsWith('/') && !modulePath.startsWith('//')), 'module path belongs to the demo');
const width = Number(widthArg);
const height = Number(heightArg);
assert.ok(Number.isInteger(width) && width >= 320 && width <= 1920, 'valid viewport width');
assert.ok(Number.isInteger(height) && height >= 480 && height <= 1200, 'valid viewport height');
const output = resolve(
    '.tmp/hass-loader-capture',
    `${packaged ? `packaged-${target.pathname === '/' ? '' : 'ingress-'}` : ''}${width}x${height}`,
);
await mkdir(output, { recursive: true });
const profile = await mkdtemp(resolve('.tmp/hc-'));
const chrome = spawn(
    process.env.CHROME_BINARY ?? 'google-chrome-stable',
    [
        '--headless=new',
        '--no-sandbox',
        '--no-first-run',
        '--no-default-browser-check',
        '--disable-background-networking',
        '--disable-extensions',
        '--disable-gpu',
        '--remote-debugging-address=127.0.0.1',
        '--remote-debugging-port=0',
        `--user-data-dir=${profile}`,
        `--window-size=${width},${height}`,
        'about:blank',
    ],
    {
        detached: true,
        stdio: ['ignore', 'ignore', 'pipe'],
        env: {
            ...process.env,
            XDG_CONFIG_HOME: profile,
            XDG_CACHE_HOME: profile,
            TMPDIR: profile,
        },
    },
);
let chromeError = '';
chrome.stderr.on('data', chunk => {
    chromeError = (chromeError + chunk.toString()).slice(-4096);
});

async function until(check, label) {
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
        try {
            const value = await check();
            if (value) return value;
        } catch {
            // Navigation can briefly destroy the current execution context.
        }
        await sleep(50);
    }
    throw Error(`timed out waiting for ${label}; Chrome exit=${chrome.exitCode ?? 'running'} ${chromeError}`);
}

class DevTools {
    constructor(socket) {
        this.socket = socket;
        this.nextId = 0;
        this.pending = new Map();
        this.listeners = new Map();
        socket.addEventListener('message', event => {
            const message = JSON.parse(event.data);
            if (!message.id) {
                this.listeners.get(message.method)?.(message.params);
                return;
            }
            const pending = this.pending.get(message.id);
            if (!pending) return;
            this.pending.delete(message.id);
            if (message.error) pending.reject(Error(message.error.message));
            else pending.resolve(message.result);
        });
        socket.addEventListener('close', () => {
            for (const pending of this.pending.values()) pending.reject(Error('Chrome DevTools disconnected'));
            this.pending.clear();
        });
    }

    on(method, listener) {
        this.listeners.set(method, listener);
    }

    call(method, params = {}) {
        const id = ++this.nextId;
        return new Promise((resolveCall, reject) => {
            const timeout = setTimeout(() => {
                this.pending.delete(id);
                reject(Error(`Chrome DevTools timed out: ${method}`));
            }, 15_000);
            this.pending.set(id, {
                resolve: value => {
                    clearTimeout(timeout);
                    resolveCall(value);
                },
                reject: error => {
                    clearTimeout(timeout);
                    reject(error);
                },
            });
            this.socket.send(JSON.stringify({ id, method, params }));
        });
    }

    async evaluate(expression) {
        const result = await this.call('Runtime.evaluate', { expression, returnByValue: true });
        if (result.exceptionDetails) throw Error(result.exceptionDetails.text);
        return result.result.value;
    }

    async onWindow(functionDeclaration, argument) {
        const window = await this.call('Runtime.evaluate', { expression: 'window' });
        const result = await this.call('Runtime.callFunctionOn', {
            objectId: window.result.objectId,
            functionDeclaration,
            arguments: [{ value: argument }],
        });
        if (result.exceptionDetails) throw Error(result.exceptionDetails.text);
    }

    async screenshot(name) {
        const result = await this.call('Page.captureScreenshot', { format: 'png' });
        const path = `${output}/${name}.png`;
        await writeFile(path, Buffer.from(result.data, 'base64'));
        return path;
    }
}

let socket;
try {
    const port = await until(async () => {
        const content = await readFile(`${profile}/DevToolsActivePort`, 'utf8');
        return Number(content.split('\n')[0]);
    }, 'Chrome DevTools port');
    const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = pages.find(candidate => candidate.type === 'page');
    assert.ok(page, 'Chrome opened a page');
    socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolveOpen, reject) => {
        socket.addEventListener('open', resolveOpen, { once: true });
        socket.addEventListener('error', reject, { once: true });
    });
    const devtools = new DevTools(socket);
    await devtools.call('Page.enable');
    await devtools.call('Runtime.enable');
    await devtools.call('Log.enable');
    await devtools.call('Emulation.setDeviceMetricsOverride', {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
    });
    if (packaged) {
        const html = await (await fetch(url)).text();
        const wasm = html.match(/<meta name="garmin-wasm" content="\.\/([^"<>]+)">/);
        const module = html.match(/<meta name="garmin-module" content="\.\/([^"<>]+)">/);
        assert.ok(wasm, 'a packaged WASM asset is advertised');
        assert.ok(module, 'a packaged application module is advertised');
        const assetPath = `/${wasm[1]}`;
        const control = new URL('/api/browser-response', controlTarget);
        async function fault(kind, milliseconds, path = assetPath) {
            const response = await fetch(control, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ kind, path, ...(milliseconds ? { milliseconds } : {}) }),
            });
            assert.equal(response.status, 200, await response.text());
        }
        async function ready(name) {
            await until(
                () => devtools.evaluate('document.getElementById("loading")?.hidden === true'),
                'application ready',
            );
            const screenshot = await until(async () => {
                const response = await fetch(new URL('/api/control', controlTarget), {
                    method: 'POST',
                    headers: { 'content-type': 'application/json' },
                    body: JSON.stringify({ operation: 'screenshot' }),
                });
                if (response.status === 404) return false;
                if (response.status !== 200) throw Error(`app screenshot failed: ${await response.text()}`);
                return response;
            }, 'app screenshot');
            assert.match(screenshot.headers.get('content-type') ?? '', /image\/png/);
            const path = `${output}/${name}.png`;
            await writeFile(path, Buffer.from(await screenshot.arrayBuffer()));
            return path;
        }
        await devtools.call('Network.enable');
        const preloadWarnings = [];
        devtools.on('Log.entryAdded', ({ entry }) => {
            if (/preload/i.test(entry.text)) preloadWarnings.push(entry.text);
        });
        await devtools.call('Network.setCacheDisabled', { cacheDisabled: true });
        await fault('delay', 2000);
        const started = Date.now();
        await devtools.call('Page.navigate', { url });
        await until(() => devtools.evaluate('!!document.getElementById("loading")'), 'packaged loader');
        const loading = await devtools.screenshot('loading');
        const initial = await ready('initial');
        assert.ok(Date.now() - started >= 1800, 'delayed WASM response held startup');
        await fault('fail');
        await devtools.call('Page.reload', { ignoreCache: true });
        await until(
            () => devtools.evaluate('document.getElementById("loading")?.dataset.state === "failure"'),
            'packaged WASM failure',
        );
        const failure = await devtools.screenshot('failure');
        const failureText = await devtools.evaluate('document.getElementById("loading-panel").innerText');
        assert.match(failureText, /could not start/);
        const panel = await devtools.evaluate(
            '(() => { const r = document.getElementById("loading-panel").getBoundingClientRect(); return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: innerWidth, height: innerHeight }; })()',
        );
        assert.ok(panel.left >= 0 && panel.right <= panel.width, 'failure panel fits the viewport width');
        assert.ok(panel.top >= 0 && panel.bottom <= panel.height, 'failure panel fits the viewport height');
        await devtools.onWindow('function() { document.getElementById("loading-retry").click(); }');
        const retry = await ready('retry');
        let cachedAssets = 0;
        devtools.on('Network.requestServedFromCache', () => cachedAssets++);
        await devtools.call('Network.setCacheDisabled', { cacheDisabled: false });
        await devtools.call('Page.reload', { ignoreCache: false });
        await ready('cache-seed');
        preloadWarnings.length = 0;
        await devtools.call('Page.reload', { ignoreCache: false });
        const cached = await ready('cached');
        await sleep(3500);
        assert.ok(cachedAssets > 0, 'packaged assets were served from browser cache');
        assert.deepEqual(preloadWarnings, [], 'warm reload has no unused preload warnings');
        await devtools.call('Network.setCacheDisabled', { cacheDisabled: true });
        await fault('fail', undefined, `/${module[1]}`);
        await devtools.call('Page.reload', { ignoreCache: true });
        await until(
            () => devtools.evaluate('document.getElementById("loading")?.dataset.state === "failure"'),
            'packaged module failure',
        );
        const moduleFailure = await devtools.screenshot('module-failure');
        await devtools.onWindow('function() { document.getElementById("loading-retry").click(); }');
        const moduleRetry = await ready('module-retry');
        console.log(
            JSON.stringify(
                { loading, initial, failure, retry, cached, cachedAssets, moduleFailure, moduleRetry, failureText },
                null,
                2,
            ),
        );
    } else {
        await devtools.call('Page.navigate', { url });
        await until(
            () => devtools.evaluate('document.readyState === "complete" && !!document.getElementById("loading")'),
            'loader',
        );
        const loading = await devtools.screenshot('loading');

        const control = new URL('/api/browser-response', controlTarget);
        const configured = await fetch(control, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ kind: 'fail', path: modulePath }),
        });
        assert.equal(configured.status, 200, await configured.text());
        await devtools.onWindow(
            'function(path) { const script = document.createElement("script"); script.type = "module"; script.src = path; document.head.append(script); }',
            modulePath,
        );
        await until(
            () => devtools.evaluate('document.getElementById("loading")?.dataset.state === "failure"'),
            'module failure',
        );
        const failure = await devtools.screenshot('failure');
        const failureText = await devtools.evaluate('document.getElementById("loading-panel").innerText');
        assert.match(failureText, /could not start/);
        assert.match(failureText, /Reload/);
        const panel = await devtools.evaluate(
            '(() => { const r = document.getElementById("loading-panel").getBoundingClientRect(); return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: innerWidth, height: innerHeight }; })()',
        );
        assert.ok(panel.left >= 0 && panel.right <= panel.width, 'failure panel fits the viewport width');
        assert.ok(panel.top >= 0 && panel.bottom <= panel.height, 'failure panel fits the viewport height');
        const recoveredAsset = await fetch(new URL(modulePath, url));
        assert.equal(recoveredAsset.status, 200, 'the response fault was consumed once');

        await devtools.onWindow('function() { document.getElementById("loading-retry").click(); }');
        await until(
            () =>
                devtools.evaluate(
                    'document.readyState === "complete" && !document.getElementById("loading")?.dataset.state',
                ),
            'reload',
        );
        const retry = await devtools.screenshot('retry');
        console.log(JSON.stringify({ loading, failure, retry, failureText }, null, 2));
    }
} finally {
    socket?.close();
    if (chrome.pid && chrome.exitCode === null && chrome.signalCode === null) {
        try {
            process.kill(-chrome.pid, 'SIGTERM');
        } catch {
            // Chrome may already have exited.
        }
        await Promise.race([new Promise(resolveExit => chrome.once('exit', resolveExit)), sleep(3_000)]);
        if (chrome.exitCode === null && chrome.signalCode === null) {
            try {
                process.kill(-chrome.pid, 'SIGKILL');
            } catch {
                // The process group has exited.
            }
        }
    }
    await rm(profile, { recursive: true, force: true });
}
