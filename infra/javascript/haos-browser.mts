// Verify the production app through HA ingress when its semantic control API is disabled.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { open, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { populatedWorkflow } from './haos-workflow.mts';
import { verifyInstallLinks } from './haos-install-links.mts';
import { DevTools } from './haos-devtools.mts';
import type { Clip, ReadySession, Session } from './haos-types.mts';

const [sessionPath, evidencePath] = process.argv.slice(2);
assert.ok(sessionPath && evidencePath, 'session and evidence paths are required');
const probe = process.argv[4] === '--probe';
const state = JSON.parse(await readFile(sessionPath, 'utf8')) as Session;
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
let socket: WebSocket | undefined;
let client: DevTools | undefined;
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

async function until<T>(
    check: () => T | Promise<T>,
    label: string,
    timeout = 90_000,
): Promise<NonNullable<Exclude<T, false | 0 | ''>>> {
    const deadline = Date.now() + timeout;
    let last: unknown;
    while (Date.now() < deadline && !ending) {
        if (chrome.exitCode !== null || chrome.signalCode !== null) throw Error('Owned Chrome exited');
        try {
            const value = await check();
            if (value) return value as NonNullable<Exclude<T, false | 0 | ''>>;
        } catch (error) {
            last = error;
        }
        await sleep(250);
    }
    throw Error(`Timed out waiting for ${label}: ${last ?? 'not ready'}`);
}

try {
    const port = await until(
        async () => Number((await readFile(`${profile}/DevToolsActivePort`, 'utf8')).split('\n')[0]),
        'debugging port',
    );
    const page = (await (
        await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, {
            method: 'PUT',
        })
    ).json()) as { webSocketDebuggerUrl: string };
    assert.ok(page, 'owned Chrome has a page');
    const connection = new WebSocket(page.webSocketDebuggerUrl);
    socket = connection;
    await new Promise((done, fail) => {
        connection.addEventListener('open', done, { once: true });
        connection.addEventListener('error', fail, { once: true });
    });
    const devtools = new DevTools(connection, evidencePath, until);
    client = devtools;
    await until(() => devtools.call('Page.enable'), 'active owned Chrome page');
    await client.call('Network.enable');
    await client.call('Emulation.setEmulatedMedia', {
        features: [{ name: 'prefers-color-scheme', value: 'dark' }],
    });
    await client.capture('browser-probe.png');
    await writeFile(resolve(evidencePath, 'browser-started.json'), '{}\n');
    if (probe) {
        console.log('Owned Chrome page and application screenshot capture are available.');
    } else {
        const readyState = await until(
            async () => {
                const current = JSON.parse(await readFile(sessionPath, 'utf8')) as ReadySession;
                return current.phase === 'verifying' ? current : null;
            },
            'disposable HAOS onboarding',
            1_800_000,
        );
        await client.call('Page.navigate', { url: state.base_url });
        await until(
            () => devtools.evaluate(() => document.readyState === 'complete' && location.hostname === '127.0.0.1'),
            'HA document',
        );
        await until(
            () =>
                devtools.evaluate(tokens => {
                    localStorage.setItem('hassTokens', JSON.stringify(tokens));
                    return true;
                }, readyState.tokens),
            'HA authentication storage',
        );
        await client.capture('before-ingress.png');
        await client.call('Page.navigate', { url: readyState.url });
        const ready = await until(
            () =>
                devtools.evaluate(() => {
                    function find(root: Document | ShadowRoot): { viewport: number[]; clip: Clip } | null {
                        for (const element of root.querySelectorAll('*')) {
                            if (
                                element instanceof HTMLIFrameElement &&
                                element.contentDocument?.querySelector('#garmin-toolkit')
                            ) {
                                const document = element.contentDocument;
                                const canvas = document.querySelector<HTMLCanvasElement>('#garmin-toolkit');
                                const loading = document.querySelector<HTMLElement>('#loading');
                                const rect = element.getBoundingClientRect();
                                if (canvas?.width && canvas.height && loading?.hidden && rect.width && rect.height) {
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
        assert.equal(remoc.method, 'Network.webSocketCreated');
        await until(
            () =>
                devtools.events.some(
                    event =>
                        event.method === 'Network.webSocketFrameReceived' &&
                        event.params.requestId === remoc.params.requestId,
                ),
            'app service response',
        );
        await client.evaluate(
            () =>
                new Promise<void>(done => {
                    requestAnimationFrame(() => requestAnimationFrame(() => done()));
                }),
        );
        await client.capture('ready.png', ready.clip);
        await client.capture('home-assistant.png');
        client.recording = false;
        client.events = [];
        await writeFile(
            resolve(evidencePath, 'ready.json'),
            JSON.stringify({
                url: readyState.url,
                ingress: true,
                websocket: true,
                viewport: ready.viewport,
                theme: 'dark',
            }),
        );
        console.log('Garmin Toolkit rendered through authenticated Home Assistant ingress.');
        if (readyState.install_links) {
            await until(
                () => readFile(resolve(evidencePath, 'install-links-prepared.json')),
                'fresh repository installation setup',
                300_000,
            );
            await verifyInstallLinks(client, readyState, evidencePath, until);
        }
        if (readyState.workflow) await populatedWorkflow(client, readyState, sessionPath, evidencePath);
        await new Promise(done => chrome.once('exit', done));
    }
} catch (error) {
    if (!ending) {
        console.error(error);
        if (client) {
            try {
                await client.capture('failed.png');
            } catch (captureError) {
                console.error('Failure screenshot unavailable:', captureError);
            }
        }
        process.exitCode = 1;
    }
} finally {
    await stop();
}
