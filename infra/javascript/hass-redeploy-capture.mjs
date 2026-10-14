// Exercise a stale packaged entrypoint across a disposable HASS redeploy.
// The two bundle variants and every browser artifact stay under .tmp.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { link, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const root = resolve('.tmp/hass-redeploy');
const bundle = resolve('.tmp/hass-packaged-web');
const output = join(root, 'evidence');
const profile = await mkdtemp(resolve('.tmp/hc-'));
const url = 'http://127.0.0.1:18100/api/hassio_ingress/test/nested/';
const control = 'http://127.0.0.1:18099/api/control';
const width = 390;
const height = 700;

async function until(check, label, timeout = 15_000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        try {
            const value = await check();
            if (value) return value;
        } catch {
            // A navigation or server restart can briefly interrupt a request.
        }
        await sleep(50);
    }
    throw Error(`timed out waiting for ${label}`);
}

function hashSuffix(bytes) {
    const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
    const digest = createHash('sha256').update(bytes).digest();
    let bits = 0;
    let value = 0;
    let result = '';
    for (const byte of digest.subarray(0, 5)) {
        value = (value << 8) | byte;
        bits += 8;
        while (bits >= 5) {
            bits -= 5;
            result += alphabet[(value >> bits) & 31];
        }
    }
    return result;
}

function renamed(path, bytes) {
    const suffix = hashSuffix(bytes);
    const next = path.replace(/-[A-Z2-7]{8}(\.[a-z]+)$/, `-${suffix}$1`);
    assert.notEqual(next, path, `new fingerprint for ${path}`);
    return next;
}

async function mirror(source, destination, omit = () => false, prefix = '') {
    await mkdir(destination, { recursive: true });
    for (const entry of await readdir(source, { withFileTypes: true })) {
        const relative = join(prefix, entry.name);
        if (entry.isDirectory()) await mirror(join(source, entry.name), join(destination, entry.name), omit, relative);
        else if (entry.isFile() && !omit(relative)) await link(join(source, entry.name), join(destination, entry.name));
        else if (!entry.isFile()) throw Error(`unsupported bundle entry: ${relative}`);
    }
}

async function prepare() {
    await mkdir(output, { recursive: true });
    const first = join(root, 'v1');
    const second = join(root, 'v2');
    const html = await readFile(join(bundle, 'index.html'), 'utf8');
    const module = html.match(/<meta name="garmin-module" content="\.\/([^"<>]+)">/)?.[1];
    const icon = html.match(/<link rel="icon" href="\.\/([^"<>]+)"/)?.[1];
    assert.ok(module && icon, 'packaged HTML declares a module and favicon');
    const moduleBytes = Buffer.concat([
        await readFile(join(bundle, module)),
        Buffer.from('\n// redeploy fixture v2\n'),
    ]);
    const originalIcon = await readFile(join(bundle, icon), 'utf8');
    const changedIcon = originalIcon.replace('#006680', '#d64f4f');
    assert.notEqual(changedIcon, originalIcon, 'fixture changes the visible favicon color');
    const newModule = renamed(module, moduleBytes);
    const newIcon = renamed(icon, changedIcon);
    await rm(first, { recursive: true, force: true });
    await rm(second, { recursive: true, force: true });
    await mirror(bundle, first);
    await mirror(
        bundle,
        second,
        path => path === 'index.html' || [module, icon].some(old => path === old || path.startsWith(`${old}.`)),
    );
    await writeFile(join(second, newModule), moduleBytes);
    await writeFile(join(second, newIcon), changedIcon);
    const nextHtml = html.replaceAll(module, newModule).replaceAll(icon, newIcon);
    assert.ok(!nextHtml.includes(module) && !nextHtml.includes(icon), 'new entrypoint drops stale references');
    await writeFile(join(second, 'index.html'), nextHtml);
    return { first, second, html, module, icon, newModule, newIcon, originalIcon, changedIcon };
}

function processGroup(command, args, environment = {}) {
    const child = spawn(command, args, {
        detached: true,
        stdio: ['ignore', 'ignore', 'pipe'],
        env: { ...process.env, TMPDIR: root, ...environment },
    });
    let error = '';
    child.stderr.on('data', chunk => {
        error = (error + chunk.toString()).slice(-4096);
    });
    return { child, error: () => error };
}

async function stop(handle) {
    if (!handle?.child.pid || handle.child.exitCode !== null || handle.child.signalCode !== null) return;
    try {
        process.kill(-handle.child.pid, 'SIGTERM');
    } catch {
        return;
    }
    await Promise.race([new Promise(done => handle.child.once('exit', done)), sleep(3000)]);
    if (handle.child.exitCode === null && handle.child.signalCode === null) {
        try {
            process.kill(-handle.child.pid, 'SIGKILL');
        } catch {
            // The process group exited during the timeout.
        }
    }
}

async function startHost(webRoot, expectedIcon) {
    const host = processGroup(resolve('.tmp/cargo-target/debug/garmin-hass'), ['--control-server', '--ui-automation'], {
        GARMIN_TOOLKIT_HASS_DATA_BASE: join(root, webRoot === join(root, 'v1') ? 'data-v1' : 'data-v2'),
        GARMIN_TOOLKIT_HASS_WEB_ROOT: webRoot,
        GARMIN_TOOLKIT_HASS_ADDRESS: '127.0.0.1:18099',
    });
    try {
        await until(async () => {
            const response = await fetch('http://127.0.0.1:18099/');
            return response.ok && (await response.text()).includes(expectedIcon);
        }, 'HASS host serving the requested deployment');
        return host;
    } catch (error) {
        await stop(host);
        throw Error(`${error.message}: ${host.error()}`);
    }
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
    }

    on(method, listener) {
        this.listeners.set(method, listener);
    }

    call(method, params = {}) {
        const id = ++this.nextId;
        return new Promise((done, fail) => {
            const timeout = setTimeout(() => {
                this.pending.delete(id);
                fail(Error(`DevTools timed out: ${method}`));
            }, 15_000);
            this.pending.set(id, {
                resolve: value => {
                    clearTimeout(timeout);
                    done(value);
                },
                reject: error => {
                    clearTimeout(timeout);
                    fail(error);
                },
            });
            this.socket.send(JSON.stringify({ id, method, params }));
        });
    }

    async evaluate(expression) {
        const reply = await this.call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
        if (reply.exceptionDetails) throw Error(reply.exceptionDetails.text);
        return reply.result.value;
    }

    async screenshot(name) {
        const reply = await this.call('Page.captureScreenshot', { format: 'png' });
        const path = join(output, `${name}.png`);
        await writeFile(path, Buffer.from(reply.data, 'base64'));
        return path;
    }
}

async function appScreenshot(name) {
    let lastResponse = '';
    const response = await until(async () => {
        const result = await fetch(control, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ operation: 'screenshot' }),
        });
        if (result.status === 200) return result;
        lastResponse = `${result.status} ${await result.text()}`;
        return false;
    }, `app control screenshot (${name})`).catch(error => {
        throw Error(`${error.message}; last response: ${lastResponse}`);
    });
    const path = join(output, `${name}.png`);
    await writeFile(path, Buffer.from(await response.arrayBuffer()));
    return path;
}

async function chromePage() {
    const chrome = processGroup(
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
        { XDG_CONFIG_HOME: profile, XDG_CACHE_HOME: profile, TMPDIR: profile },
    );
    try {
        const port = await until(
            async () => Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]),
            'Chrome port',
        );
        const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
        const page = pages.find(item => item.type === 'page');
        assert.ok(page, 'Chrome opened a page');
        const socket = new WebSocket(page.webSocketDebuggerUrl);
        await new Promise((done, fail) => {
            socket.addEventListener('open', done, { once: true });
            socket.addEventListener('error', fail, { once: true });
        });
        const devtools = new DevTools(socket);
        await devtools.call('Page.enable');
        await devtools.call('Runtime.enable');
        await devtools.call('Network.enable');
        await devtools.call('Emulation.setDeviceMetricsOverride', {
            width,
            height,
            deviceScaleFactor: 1,
            mobile: false,
        });
        return { chrome, socket, devtools };
    } catch (error) {
        await stop(chrome);
        throw Error(`${error.message}: ${chrome.error()}`);
    }
}

let host;
let proxy;
let browser;
try {
    const cgroup = (await readFile('/proc/self/cgroup', 'utf8')).trim().match(/^0::(\/.*)$/)?.[1];
    assert.ok(cgroup?.startsWith('/user.slice/'), 'run in a systemd user scope');
    const limits = join('/sys/fs/cgroup', cgroup);
    const memory = Number((await readFile(join(limits, 'memory.max'), 'utf8')).trim());
    const [quota, period] = (await readFile(join(limits, 'cpu.max'), 'utf8')).trim().split(' ').map(Number);
    assert.ok(memory <= 6 * 1024 ** 3 && quota / period <= 2, 'run inside the 6 GiB, 2 CPU scope');
    const fixture = await prepare();
    host = await startHost(fixture.first, fixture.icon);
    proxy = processGroup(process.execPath, [
        'infra/javascript/hass-ingress-proxy.mjs',
        '18100',
        '/api/hassio_ingress/test/nested/',
        '18099',
    ]);
    await until(async () => (await fetch(url)).ok, 'nested ingress proxy');
    browser = await chromePage();
    const { devtools } = browser;
    await devtools.call('Page.navigate', { url });
    await until(() => devtools.evaluate('document.getElementById("loading")?.hidden === true'), 'first app');
    const before = await appScreenshot('before');
    const iconBefore = await devtools.evaluate(
        '(async () => { const href = document.querySelector("link[rel=icon]").href; const response = await fetch(href); return { href, status: response.status, body: await response.text() }; })()',
    );
    assert.equal(iconBefore.status, 200);
    assert.ok(iconBefore.href.endsWith(fixture.icon));
    const indexBefore = await fetch(url);
    assert.equal(indexBefore.headers.get('cache-control'), 'no-store');
    const staleHtml = await indexBefore.text();
    await stop(host);
    host = await startHost(fixture.second, fixture.newIcon);
    const oldModule = await fetch(new URL(fixture.module, url), { cache: 'no-store' });
    assert.equal(
        oldModule.status,
        404,
        `old module is absent after deployment (${oldModule.headers.get('content-type')})`,
    );
    await devtools.call('Network.clearBrowserCache');
    await devtools.call('Network.setCacheDisabled', { cacheDisabled: true });
    let fulfilled = false;
    devtools.on('Fetch.requestPaused', async ({ requestId }) => {
        if (fulfilled) return devtools.call('Fetch.continueRequest', { requestId });
        fulfilled = true;
        await devtools.call('Fetch.fulfillRequest', {
            requestId,
            responseCode: 200,
            responseHeaders: [{ name: 'content-type', value: 'text/html; charset=utf-8' }],
            body: Buffer.from(staleHtml).toString('base64'),
        });
        await devtools.call('Fetch.disable');
    });
    await devtools.call('Fetch.enable', { patterns: [{ urlPattern: url, resourceType: 'Document' }] });
    await devtools.call('Page.navigate', { url });
    await until(
        () => devtools.evaluate('document.getElementById("loading")?.dataset.state === "failure"'),
        'stale entrypoint failure',
    );
    assert.ok(fulfilled, 'one old HTML response was served');
    const failureText = await devtools.evaluate('document.getElementById("loading-panel").innerText');
    assert.match(failureText, /required application file could not load/);
    const stale = await devtools.screenshot('stale');
    await devtools.evaluate('document.getElementById("loading-retry").click()');
    await until(() => devtools.evaluate('document.getElementById("loading")?.hidden === true'), 'redeployed app');
    const after = await appScreenshot('after');
    const iconAfter = await devtools.evaluate(
        '(async () => { const href = document.querySelector("link[rel=icon]").href; const response = await fetch(href); return { href, status: response.status, body: await response.text() }; })()',
    );
    assert.equal(iconAfter.status, 200);
    assert.ok(iconAfter.href.endsWith(fixture.newIcon));
    assert.equal(
        await devtools.evaluate('document.querySelector("meta[name=garmin-module]").content'),
        `./${fixture.newModule}`,
    );
    assert.notEqual(iconAfter.href, iconBefore.href);
    assert.notEqual(iconAfter.body, iconBefore.body);
    const indexAfter = await fetch(url);
    assert.equal(indexAfter.headers.get('cache-control'), 'no-store');
    assert.ok((await indexAfter.text()).includes(fixture.newIcon));
    await devtools.call('Emulation.setDeviceMetricsOverride', {
        width: 800,
        height: 320,
        deviceScaleFactor: 1,
        mobile: false,
    });
    const iconPage = `<html><meta charset="utf-8"><style>body{font:16px system-ui;background:#10151b;color:#f4f7fa;display:flex;gap:48px;padding:32px}section{display:grid;gap:16px;padding:24px;background:#222b35}img{width:96px;height:96px}</style><section>Before<img src="data:image/svg+xml;base64,${Buffer.from(iconBefore.body).toString('base64')}"></section><section>After<img src="data:image/svg+xml;base64,${Buffer.from(iconAfter.body).toString('base64')}"></section></html>`;
    await devtools.call('Page.navigate', { url: `data:text/html;base64,${Buffer.from(iconPage).toString('base64')}` });
    await until(
        () =>
            devtools.evaluate(
                'document.readyState === "complete" && [...document.images].every(image => image.complete)',
            ),
        'favicon evidence',
    );
    const favicons = await devtools.screenshot('favicons');
    console.log(
        JSON.stringify(
            {
                before,
                stale,
                after,
                favicons,
                oldModuleStatus: oldModule.status,
                oldIcon: fixture.icon,
                newIcon: fixture.newIcon,
                failureText,
            },
            null,
            2,
        ),
    );
} finally {
    browser?.socket.close();
    await stop(browser?.chrome);
    await stop(proxy);
    await stop(host);
    await rm(profile, { recursive: true, force: true });
    for (const directory of ['v1', 'v2', 'data-v1', 'data-v2']) {
        await rm(join(root, directory), { recursive: true, force: true });
    }
}
