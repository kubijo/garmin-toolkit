// Run Chrome DevTools MCP against an isolated, disposable Chrome profile.
// Point an MCP client at this script instead of using --autoConnect.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const probe = process.argv.includes('--probe');
const tempRoot = fileURLToPath(new URL('../../.tmp', import.meta.url));
const npmCache = resolve(tempRoot, 'cache', 'chrome-mcp-npm');
await mkdir(tempRoot, { recursive: true });
const profile = await mkdtemp(resolve(tempRoot, 'chrome-mcp-'));
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
    chromeError = (chromeError + chunk.toString()).slice(-2048);
});

let server;
let stopping = false;
const running = child => child?.pid && child.exitCode === null && child.signalCode === null;
function signalGroup(child, signal) {
    if (!child?.pid) return;
    try {
        process.kill(-child.pid, signal);
    } catch {
        // The process group has already exited.
    }
}
async function stop(code = 0) {
    if (stopping) return;
    stopping = true;
    signalGroup(server, 'SIGTERM');
    signalGroup(chrome, 'SIGTERM');
    const exited = Promise.all([
        new Promise(done => (running(chrome) ? chrome.once('exit', done) : done())),
        new Promise(done => (running(server) ? server.once('exit', done) : done())),
    ]);
    const forced = setTimeout(() => {
        signalGroup(server, 'SIGKILL');
        signalGroup(chrome, 'SIGKILL');
    }, 3000);
    forced.unref();
    try {
        await exited;
    } finally {
        clearTimeout(forced);
    }
    await rm(profile, { recursive: true, force: true });
    process.exitCode = code;
}

chrome.on('error', error => {
    console.error(error);
    void stop(1);
});
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => {
        void stop(0);
    });
}
process.stdin.on('end', () => {
    void stop(0);
});

try {
    const deadline = Date.now() + 15_000;
    let port;
    while (Date.now() < deadline) {
        if (!running(chrome)) throw Error(`Chrome exited: ${chromeError}`);
        try {
            const content = await readFile(resolve(profile, 'DevToolsActivePort'), 'utf8');
            port = Number(content.split('\n')[0]);
            if (Number.isInteger(port) && port > 0) break;
        } catch {
            // Chrome has not opened its debugging listener yet.
        }
        await sleep(50);
    }
    assert.ok(port, `Chrome debugging listener timed out: ${chromeError}`);
    const browserUrl = `http://127.0.0.1:${port}`;
    const version = await (await fetch(`${browserUrl}/json/version`)).json();
    assert.ok(version.webSocketDebuggerUrl, 'Chrome exposes a browser debugger');

    const executable = process.env.CHROME_MCP_CLI;
    if (!executable) await mkdir(npmCache, { recursive: true });
    server = spawn(
        executable ? process.execPath : 'npx',
        [
            ...(executable ? [executable] : ['--yes', 'chrome-devtools-mcp@latest']),
            `--browser-url=${browserUrl}`,
            '--no-performance-crux',
            '--no-usage-statistics',
        ],
        {
            detached: true,
            stdio: probe ? ['pipe', 'pipe', 'inherit'] : 'inherit',
            env: {
                ...process.env,
                XDG_CONFIG_HOME: profile,
                XDG_CACHE_HOME: profile,
                TMPDIR: profile,
                npm_config_cache: npmCache,
            },
        },
    );
    server.on('error', error => {
        console.error(error);
        void stop(1);
    });
    server.on('exit', code => {
        void stop(code ?? 1);
    });

    if (probe) {
        const response = new Promise((done, fail) => {
            let output = '';
            const timeout = setTimeout(() => fail(Error('MCP initialize timed out')), 10_000);
            server.stdout.on('data', chunk => {
                output += chunk.toString();
                for (const line of output.split('\n')) {
                    if (!line.startsWith('{')) continue;
                    try {
                        const message = JSON.parse(line);
                        if (message.id === 1) {
                            clearTimeout(timeout);
                            done(message);
                        }
                    } catch {
                        // Wait for a complete response line.
                    }
                }
            });
        });
        const initialize = JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'initialize',
            params: {
                protocolVersion: '2025-03-26',
                capabilities: {},
                clientInfo: { name: 'probe', version: '1' },
            },
        });
        server.stdin.write(`${initialize}\n`);
        const message = await response;
        assert.ok(message.result?.serverInfo, 'MCP initialized');
        console.error(`MCP attached to isolated Chrome: ${message.result.serverInfo.name}`);
        await stop(0);
    }
} catch (error) {
    console.error(error);
    await stop(1);
}
