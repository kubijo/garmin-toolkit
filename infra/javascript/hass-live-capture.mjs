// Measure a Rust UI edit through the live Trunk server without changing checked-out source.
// Run while a capped demo host and Trunk server use the disposable ports below.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const root = resolve('.tmp/hass-live-probe');
const source = resolve('crates/garmin-ui/src/profile.rs');
const fixture = join(root, 'ui-src', 'profile.rs');
const url = 'http://127.0.0.1:18101/';
const control = 'http://127.0.0.1:18099/api/control';
const originalLabel = 'default_message: "Create a profile",\n    );\n    let owner';
const editedLabel = 'default_message: "Create a profile LIVE",\n    );\n    let owner';
const reverse = process.argv[2] === 'reverse';
await mkdir(root, { recursive: true });
if (process.argv[2] === 'prepare') {
    await rm(join(root, 'ui-src'), { recursive: true, force: true });
    await cp(resolve('crates/garmin-ui/src'), join(root, 'ui-src'), { recursive: true });
    console.log(`Disposable UI source copy: ${join(root, 'ui-src')}`);
    process.exit(0);
}
const profile = await mkdtemp(resolve('.tmp/hc-'));

async function until(check, label, timeout = 45_000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
        try {
            const result = await check();
            if (result) return result;
        } catch {
            // Browser navigation and server rebuilds briefly interrupt requests.
        }
        await sleep(50);
    }
    throw Error(`timed out waiting for ${label}`);
}

class DevTools {
    constructor(socket) {
        this.socket = socket;
        this.id = 0;
        this.pending = new Map();
        this.mainNavigations = 0;
        this.issues = [];
        socket.addEventListener('message', event => {
            const message = JSON.parse(event.data);
            if (message.method === 'Page.frameNavigated' && !message.params.frame.parentId) this.mainNavigations++;
            if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error')
                this.issues.push(message.params.entry.text);
            if (message.method === 'Network.loadingFailed') this.issues.push(message.params.errorText);
            if (!message.id) return;
            const pending = this.pending.get(message.id);
            if (!pending) return;
            this.pending.delete(message.id);
            if (message.error) pending.reject(Error(message.error.message));
            else pending.resolve(message.result);
        });
    }

    call(method, params = {}) {
        const id = ++this.id;
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
        const reply = await this.call('Runtime.evaluate', { expression, returnByValue: true });
        if (reply.exceptionDetails) throw Error(reply.exceptionDetails.text);
        return reply.result.value;
    }
}

async function screenshot(name) {
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
    }, `app screenshot (${name})`).catch(error => {
        throw Error(`${error.message}; last response: ${lastResponse}`);
    });
    const image = Buffer.from(await response.arrayBuffer());
    const path = join(root, `${name}.png`);
    await writeFile(path, image);
    return { path, hash: createHash('sha256').update(image).digest('hex') };
}

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
        '--window-size=390,700',
        'about:blank',
    ],
    {
        detached: true,
        stdio: ['ignore', 'ignore', 'pipe'],
        env: { ...process.env, XDG_CONFIG_HOME: profile, XDG_CACHE_HOME: profile, TMPDIR: profile },
    },
);
let chromeError = '';
chrome.stderr.on('data', chunk => {
    chromeError = (chromeError + chunk.toString()).slice(-2048);
});
let socket;
try {
    const cgroup = (await readFile('/proc/self/cgroup', 'utf8')).trim().match(/^0::(\/.*)$/)?.[1];
    assert.ok(cgroup?.startsWith('/user.slice/'), 'run in a capped systemd user scope');
    const limits = join('/sys/fs/cgroup', cgroup);
    const memory = Number((await readFile(join(limits, 'memory.max'), 'utf8')).trim());
    const [quota, period] = (await readFile(join(limits, 'cpu.max'), 'utf8')).trim().split(' ').map(Number);
    assert.ok(memory <= 3 * 1024 ** 3 && quota / period <= 1, 'Chrome scope is limited to 1 CPU and 3 GiB');
    const original = await readFile(source, 'utf8');
    const mounted = await readFile(fixture, 'utf8');
    assert.equal(
        mounted,
        reverse ? original.replace(originalLabel, editedLabel) : original,
        'temporary source starts with the expected label',
    );
    assert.ok(original.includes(originalLabel), 'chooser label is present');
    const sourceHash = createHash('sha256').update(original).digest('hex');
    const port = await until(
        async () => Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]),
        'Chrome port',
    );
    const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = pages.find(item => item.type === 'page');
    assert.ok(page, 'Chrome opened a page');
    socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((done, fail) => {
        socket.addEventListener('open', done, { once: true });
        socket.addEventListener('error', fail, { once: true });
    });
    const devtools = new DevTools(socket);
    await devtools.call('Page.enable');
    await devtools.call('Runtime.enable');
    await devtools.call('Network.enable');
    await devtools.call('Log.enable');
    await devtools.call('Emulation.setDeviceMetricsOverride', {
        width: 390,
        height: 700,
        deviceScaleFactor: 1,
        mobile: false,
    });
    await devtools.call('Page.navigate', { url });
    await until(
        () => devtools.evaluate('document.getElementById("loading")?.hidden === true'),
        'live app ready',
        15_000,
    ).catch(async error => {
        const html = await devtools.evaluate('document.body.innerText');
        const image = await devtools.call('Page.captureScreenshot', { format: 'png' });
        await writeFile(join(root, 'startup-failure.png'), Buffer.from(image.data, 'base64'));
        throw Error(`${error.message}; page: ${html}; browser errors: ${devtools.issues.join(' | ')}`);
    });
    const before = await screenshot(reverse ? 'before-reverse' : 'before');
    const navigation = devtools.mainNavigations;
    const started = process.hrtime.bigint();
    await writeFile(fixture, reverse ? original : original.replace(originalLabel, editedLabel));
    await until(() => devtools.mainNavigations > navigation, 'Trunk reload after temporary Rust edit');
    await until(() => devtools.evaluate('document.getElementById("loading")?.hidden === true'), 'edited app ready');
    const after = await screenshot(reverse ? 'after-reverse' : 'after');
    const visibleMilliseconds = Number(process.hrtime.bigint() - started) / 1e6;
    assert.notEqual(after.hash, before.hash, 'the edited UI changed the app screenshot');
    assert.equal(
        createHash('sha256')
            .update(await readFile(source))
            .digest('hex'),
        sourceHash,
        'checked-out source is unchanged',
    );
    console.log(
        JSON.stringify(
            { before: before.path, after: after.path, visibleMilliseconds, navigationCount: devtools.mainNavigations },
            null,
            2,
        ),
    );
} catch (error) {
    throw Error(`${error.message}; Chrome: ${chromeError}`);
} finally {
    socket?.close();
    if (chrome.pid && chrome.exitCode === null && chrome.signalCode === null) {
        try {
            process.kill(-chrome.pid, 'SIGTERM');
        } catch {
            // Chrome may have exited during the request.
        }
        await Promise.race([new Promise(done => chrome.once('exit', done)), sleep(3000)]);
        if (chrome.exitCode === null && chrome.signalCode === null) {
            try {
                process.kill(-chrome.pid, 'SIGKILL');
            } catch {
                // The process group exited during the timeout.
            }
        }
    }
    await rm(profile, { recursive: true, force: true });
}
