// Verify the production app through HA ingress when its semantic control API is disabled.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { open, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { populatedWorkflow } from './haos-workflow.mjs';

const [sessionPath, evidencePath] = process.argv.slice(2);
const probe = process.argv[4] === '--probe';
let state = JSON.parse(await readFile(sessionPath, 'utf8'));
const origin = new URL(state.base_url);
assert.equal(origin.hostname, '127.0.0.1');
const profile = resolve(sessionPath, '..', 'browser-profile');
await rm(`${profile}/DevToolsActivePort`, { force: true });
const chromeLog = await open(resolve(evidencePath, 'chrome.log'), 'w');
const chrome = spawn(
    process.env.CHROME_BINARY ?? 'google-chrome',
    [
        '--headless=new',
        '--force-dark-mode',
        '--no-first-run',
        '--no-default-browser-check',
        '--disable-background-networking',
        '--disable-extensions',
        '--remote-debugging-address=127.0.0.1',
        '--remote-debugging-port=0',
        `--user-data-dir=${profile}`,
        '--window-size=1280,960',
        'about:blank',
    ],
    {
        stdio: ['ignore', 'ignore', chromeLog.fd],
        env: {
            ...process.env,
            TMPDIR: state.browser_temporary,
            XDG_CONFIG_HOME: resolve(profile, 'config'),
            XDG_CACHE_HOME: resolve(profile, 'cache'),
        },
    },
);
await chromeLog.close();
let socket;
let client;
let ending = false;
async function stop() {
    if (ending) return;
    ending = true;
    socket?.close();
    chrome.kill('SIGTERM');
    const forced = setTimeout(() => chrome.kill('SIGKILL'), 3000);
    forced.unref();
    if (chrome.exitCode === null && chrome.signalCode === null) await new Promise(done => chrome.once('exit', done));
    clearTimeout(forced);
}
for (const name of ['SIGTERM', 'SIGINT', 'SIGHUP']) process.on(name, () => void stop());

async function until(check, label, timeout = 90_000) {
    const deadline = Date.now() + timeout;
    let last;
    while (Date.now() < deadline && !ending) {
        if (chrome.exitCode !== null || chrome.signalCode !== null) throw Error('Owned Chrome exited');
        try {
            const value = await check();
            if (value) return value;
        } catch (error) {
            last = error;
        }
        await sleep(250);
    }
    throw Error(`Timed out waiting for ${label}: ${last ?? 'not ready'}`);
}

class DevTools {
    nextId = 0;
    pending = new Map();
    events = [];
    contexts = new Map();
    recording = true;
    constructor(connection) {
        this.socket = connection;
        connection.addEventListener('message', event => {
            const message = JSON.parse(event.data);
            if (!message.id) {
                if (message.method === 'Runtime.executionContextCreated')
                    this.contexts.set(message.params.context.id, message.params.context);
                if (message.method === 'Runtime.executionContextDestroyed')
                    this.contexts.delete(message.params.executionContextId);
                if (message.method === 'Runtime.executionContextsCleared') this.contexts.clear();
                if (message.method === 'Page.fileChooserOpened') this.events.push(message);
                else if (this.recording && message.method.startsWith('Network.webSocket'))
                    this.events.push({
                        method: message.method,
                        params: {
                            requestId: message.params.requestId,
                            url: message.params.url,
                        },
                    });
                return;
            }
            const pending = this.pending.get(message.id);
            if (!pending) return;
            this.pending.delete(message.id);
            clearTimeout(pending.timer);
            if (message.error) pending.reject(Error(`${pending.method}: ${message.error.message}`));
            else pending.resolve(message.result);
        });
        connection.addEventListener('close', () => {
            for (const pending of this.pending.values()) {
                clearTimeout(pending.timer);
                pending.reject(Error('Owned Chrome disconnected'));
            }
            this.pending.clear();
        });
    }
    call(method, params = {}) {
        const id = ++this.nextId;
        return new Promise((resolveCall, reject) => {
            const timer = setTimeout(() => {
                this.pending.delete(id);
                reject(Error(`DevTools command timed out: ${method}`));
            }, 15_000);
            this.pending.set(id, { resolve: resolveCall, reject, timer, method });
            this.socket.send(JSON.stringify({ id, method, params }));
        });
    }
    async evaluate(fn, argument, contextId) {
        const window = await this.call('Runtime.evaluate', {
            expression: 'window',
            ...(contextId === undefined ? {} : { contextId }),
        });
        if (window.exceptionDetails)
            throw Error(
                window.exceptionDetails.exception?.description ??
                    window.exceptionDetails.exception?.value ??
                    window.exceptionDetails.text,
            );
        assert.ok(window.result.objectId, 'the requested browser window is available');
        const reply = await this.call('Runtime.callFunctionOn', {
            objectId: window.result.objectId,
            functionDeclaration: fn.toString(),
            arguments: [{ value: argument }],
            returnByValue: true,
            awaitPromise: true,
            userGesture: true,
        });
        if (reply.exceptionDetails)
            throw Error(
                reply.exceptionDetails.exception?.description ??
                    reply.exceptionDetails.exception?.value ??
                    reply.exceptionDetails.text,
            );
        return reply.result.value;
    }
    async capture(name, clip) {
        const result = await until(
            () =>
                this.call('Page.captureScreenshot', {
                    format: 'png',
                    ...(clip ? { clip: { ...clip, scale: 1 } } : {}),
                }),
            'active page screenshot',
            30_000,
        );
        await writeFile(resolve(evidencePath, name), Buffer.from(result.data, 'base64'));
    }
}

try {
    const port = await until(
        async () => Number((await readFile(`${profile}/DevToolsActivePort`, 'utf8')).split('\n')[0]),
        'debugging port',
    );
    const page = await (
        await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, {
            method: 'PUT',
        })
    ).json();
    assert.ok(page, 'owned Chrome has a page');
    socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((done, fail) => {
        socket.addEventListener('open', done, { once: true });
        socket.addEventListener('error', fail, { once: true });
    });
    client = new DevTools(socket);
    await until(() => client.call('Page.enable'), 'active owned Chrome page');
    await client.call('Network.enable');
    await client.call('Emulation.setEmulatedMedia', {
        features: [{ name: 'prefers-color-scheme', value: 'dark' }],
    });
    await client.capture('browser-probe.png');
    await writeFile(resolve(evidencePath, 'browser-started.json'), '{}\n');
    if (probe) {
        console.log('Owned Chrome page and application screenshot capture are available.');
    } else {
        state = await until(
            async () => {
                const current = JSON.parse(await readFile(sessionPath, 'utf8'));
                return current.phase === 'verifying' ? current : null;
            },
            'disposable HAOS onboarding',
            1_800_000,
        );
        await client.call('Page.navigate', { url: state.base_url });
        await until(
            () => client.evaluate(() => document.readyState === 'complete' && location.hostname === '127.0.0.1'),
            'HA document',
        );
        await until(
            () =>
                client.evaluate(tokens => {
                    localStorage.setItem('hassTokens', JSON.stringify(tokens));
                    return true;
                }, state.tokens),
            'HA authentication storage',
        );
        await client.capture('before-ingress.png');
        await client.call('Page.navigate', { url: state.url });
        const ready = await until(
            () =>
                client.evaluate(() => {
                    function find(root) {
                        for (const element of root.querySelectorAll('*')) {
                            if (
                                element.tagName === 'IFRAME' &&
                                element.contentDocument?.querySelector('#garmin-toolkit')
                            ) {
                                const document = element.contentDocument;
                                const canvas = document.querySelector('#garmin-toolkit');
                                const loading = document.querySelector('#loading');
                                const rect = element.getBoundingClientRect();
                                if (canvas.width && canvas.height && loading?.hidden && rect.width && rect.height) {
                                    return {
                                        viewport: [window.innerWidth, window.innerHeight],
                                        clip: {
                                            x: rect.x,
                                            y: rect.y,
                                            width: rect.width,
                                            height: rect.height,
                                        },
                                    };
                                }
                            }
                            if (element.shadowRoot) {
                                const result = find(element.shadowRoot);
                                if (result) return result;
                            }
                        }
                        return null;
                    }
                    return find(document);
                }),
            'Garmin canvas through HA ingress',
        );
        const remoc = client.events.find(
            event => event.method === 'Network.webSocketCreated' && event.params.url.includes('/remoc'),
        );
        assert.ok(remoc, 'the app opened its Remoc connection through ingress');
        await until(
            () =>
                client.events.some(
                    event =>
                        event.method === 'Network.webSocketFrameReceived' &&
                        event.params.requestId === remoc.params.requestId,
                ),
            'app service response',
        );
        await client.evaluate(
            () =>
                new Promise(done => {
                    requestAnimationFrame(() => requestAnimationFrame(done));
                }),
        );
        await client.capture('ready.png', ready.clip);
        await client.capture('home-assistant.png');
        client.recording = false;
        client.events = [];
        await writeFile(
            resolve(evidencePath, 'ready.json'),
            JSON.stringify({
                url: state.url,
                ingress: true,
                websocket: true,
                viewport: ready.viewport,
                theme: 'dark',
            }),
        );
        console.log('Garmin Toolkit rendered through authenticated Home Assistant ingress.');
        if (state.workflow) await populatedWorkflow(client, state, sessionPath, evidencePath);
        await new Promise(done => chrome.once('exit', done));
    }
} catch (error) {
    if (!ending) {
        console.error(error);
        if (client) {
            try {
                await client.capture('failed.png');
            } catch (captureError) {
                console.error('Failure screenshot unavailable:', captureError.message);
            }
        }
        process.exitCode = 1;
    }
} finally {
    await stop();
}
