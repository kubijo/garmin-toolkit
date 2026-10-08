import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { closeSync, openSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { connect } from 'node:net';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { installFaults } from './faults.mjs';
import { Storage } from './storage.mjs';

export const origin = 'http://127.0.0.1:8099/';
export const click = target => ({ kind: 'click', target });
export const wait = target => ({ kind: 'wait', target });
export const scroll = (target, delta) => ({ kind: 'scroll', target, delta });

export async function eventually(check, description, timeout = 15000) {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
        if (await check()) return;
        await delay(50);
    }
    throw new Error(`Timed out: ${description}`);
}

export function listening() {
    return new Promise(resolve => {
        const socket = connect(8099, '127.0.0.1');
        const done = value => {
            socket.destroy();
            resolve(value);
        };
        socket.once('connect', () => done(true));
        socket.once('error', () => done(false));
        socket.setTimeout(500, () => done(false));
    });
}

function groupExists(pid) {
    try {
        process.kill(-pid, 0);
        return true;
    } catch (error) {
        if (error.code === 'ESRCH') return false;
        throw error;
    }
}

export async function run(page, actions) {
    actions = actions.flatMap(action => (action.kind === 'click' ? [wait(action.target), action] : [action]));
    await page.bringToFront();
    await page.waitForFunction(() => !!window.garminAutomation, null, { polling: 50, timeout: 30000 });
    await page.evaluate(actions => window.garminAutomation.sequence(actions, { run_in_background: true }), actions);
    await page.waitForFunction(
        () => {
            const state = window.garminAutomation.status()?.state;
            return state && !['running', 'paused'].includes(state);
        },
        null,
        { polling: 50, timeout: 35000 },
    );
    const report = await page.evaluate(() => {
        const { state, completed, total, failure } = window.garminAutomation.status();
        return { state, completed, total, failure };
    });
    assert.equal(
        report.state,
        'passed',
        JSON.stringify({ actions, report, targets: report.state === 'passed' ? undefined : await targets(page) }),
    );
    assert.equal(report.completed, report.total);
    await page.waitForFunction(() => !window.garminAutomation.status()?.needs_background_frames, null, {
        polling: 50,
        timeout: 10000,
    });
}

export async function targets(page) {
    return page.evaluate(() => window.garminAutomation.targets());
}

export async function selectProfile(page, index = 0) {
    await run(page, [
        wait(`profile.${index}`),
        click(`profile.${index}`),
        wait('navigation.1'),
        click('navigation.1'),
        wait('routes.import'),
    ]);
}

export async function switchProfile(page, index) {
    await run(page, [click('profile.toggle'), wait('profile.logout'), click('profile.logout')]);
    await selectProfile(page, index);
}

export async function upload(page, file, target = 'routes.import') {
    const [chooser] = await Promise.all([page.waitForEvent('filechooser'), run(page, [click(target)])]);
    await chooser.setFiles(file);
}

export async function download(page, target, directory) {
    await run(page, [wait(target)]);
    const [item] = await Promise.all([page.waitForEvent('download'), run(page, [click(target)])]);
    const path = join(directory, item.suggestedFilename());
    await item.saveAs(path);
    assert.equal(await item.failure(), null);
    return path;
}

export async function openRoute(page, name) {
    const target = (await targets(page)).find(t => t.id.startsWith('routes.open.') && t.label.startsWith(`${name} ·`));
    assert(target, `Route missing: ${name}`);
    await run(page, [click(target.id), wait('routes.title'), wait('routes.back')]);
    assert.equal((await targets(page)).find(t => t.id === 'routes.title').value, name);
}

export async function review(page, file, name) {
    await upload(page, file);
    const candidate = 'routes.candidate.TrackSegment { track: 0, segment: 0 }';
    await run(page, [wait(candidate), click(candidate), wait('routes.name')]);
    assert.equal((await targets(page)).find(t => t.id === 'routes.save').enabled, false);
    const nameBounds = (await targets(page)).find(t => t.id === 'routes.name').bounds;
    await page.mouse.click((nameBounds[0] + nameBounds[2]) / 2, (nameBounds[1] + nameBounds[3]) / 2);
    await page.waitForFunction(() => document.activeElement instanceof HTMLInputElement, null, { polling: 50 });
    await page.keyboard.press('Control+A');
    await page.keyboard.type(name);
    await page.waitForFunction(
        value => window.garminAutomation.targets().find(target => target.id === 'routes.name')?.value === value,
        name,
        { polling: 50 },
    );
    await run(page, [
        { kind: 'assert_value', target: 'routes.name', value: name },
        click('routes.sport.walking'),
        scroll('routes.name', -1200),
        wait('routes.save'),
    ]);
}

export async function prepareGeneration(page) {
    await run(page, [scroll('routes.title', -1200), wait('routes.generate')]);
}

export async function courseVisible(page, course) {
    await run(page, [wait('routes.title'), scroll('routes.title', -1200), wait(`routes.download.${course.id}`)]);
}

export class Fixture {
    constructor(name, { faults = false, tracing = true, application = true, transport = 'auto' } = {}) {
        assert.equal(process.env.GARMIN_INTEGRATION_VM, '1', 'Run this suite through the disposable VM recipe');
        this.name = name;
        this.faults = faults;
        this.tracing = tracing;
        this.application = application;
        this.transport = transport;
        this.children = [];
        this.log = [];
        this.storage = null;
    }

    async start() {
        assert.equal(await listening(), false, 'Refusing to use an existing server');
        const root = process.env.GARMIN_E2E_STATE_ROOT;
        const output = process.env.GARMIN_E2E_RESULTS;
        assert(root && output);
        await mkdir(root, { recursive: true });
        this.work = await mkdtemp(join(root, 'case-'));
        this.output = join(output, this.name);
        await mkdir(this.output, { recursive: true });
        this.storage = new Storage(this.work);
        await this.startHost();
        const { chromium } = await import(pathToFileURL(join(process.env.GARMIN_PLAYWRIGHT, 'index.mjs')).href);
        this.server = await chromium.launchServer({
            headless: false,
            args: ['--use-gl=angle', '--use-angle=gl', '--ignore-gpu-blocklist'],
        });
        this.server
            .process()
            .stderr?.on('data', data => this.log.push({ type: 'browser-process', text: data.toString() }));
        this.browser = await chromium.connect(this.server.wsEndpoint());
        this.context = await this.browser.newContext({ viewport: { width: 1280, height: 960 }, acceptDownloads: true });
        if (this.tracing) await this.context.tracing.start({ screenshots: true, snapshots: true, sources: true });
        await this.context.addInitScript(installFaults, { enabled: this.faults, transport: this.transport });
        await this.context.route('**/*', route => {
            const url = new URL(route.request().url());
            return url.origin === origin.slice(0, -1) || ['blob:', 'data:'].includes(url.protocol)
                ? route.continue()
                : route.abort();
        });
        this.page = await this.newPage();
        if (this.application) {
            await selectProfile(this.page);
            assert.equal(
                await this.page.evaluate(() => window.integrationSockets.stream),
                0,
                'Application must not select the experimental stream API',
            );
        }
        assert.deepEqual(this.storage.counts(), { profiles: 3, activities: 8, routes: 3, courses: 0 });
    }

    async startHost() {
        assert.equal(await listening(), false);
        const log = openSync(join(this.output, 'host.log'), 'a');
        try {
            this.host = spawn(process.env.GARMIN_HASS_BINARY, ['--ui-automation', '--control-server'], {
                env: {
                    ...process.env,
                    GARMIN_TOOLKIT_HASS_DATA_BASE: this.work,
                    GARMIN_E2E_GATE_ROOT: join(this.work, 'gates'),
                },
                stdio: ['ignore', log, log],
                detached: true,
            });
            this.children.push(this.host);
            this.host.on('error', error => {
                this.spawnError = error;
            });
        } finally {
            closeSync(log);
        }
        await eventually(
            async () => {
                if (this.spawnError) throw this.spawnError;
                assert.equal(this.host.exitCode, null, 'Host exited during startup');
                return listening();
            },
            'host startup',
            30000,
        );
    }

    async armGate(name) {
        const root = join(this.work, 'gates');
        await mkdir(root, { recursive: true });
        await rm(join(root, `${name}.ready`), { force: true });
        await rm(join(root, `${name}.release`), { force: true });
        await writeFile(join(root, `${name}.arm`), 'armed');
    }

    async waitGate(name) {
        await eventually(
            async () => {
                try {
                    await readFile(join(this.work, 'gates', `${name}.ready`));
                    return true;
                } catch (error) {
                    if (error.code === 'ENOENT') return false;
                    throw error;
                }
            },
            `${name} checkpoint`,
            30000,
        );
    }

    async releaseGate(name) {
        await writeFile(join(this.work, 'gates', `${name}.release`), 'released');
    }

    async stopHost(signal = 'SIGTERM') {
        if (!this.host?.pid) return;
        const child = this.host;
        const exited = child.exitCode !== null || child.signalCode !== null;
        if (!exited) {
            const exit = once(child, 'exit');
            process.kill(-child.pid, signal);
            let timer;
            try {
                await Promise.race([
                    exit,
                    new Promise((_, reject) => {
                        timer = setTimeout(() => reject(new Error('Host did not exit')), 5000);
                    }),
                ]);
            } catch {
                process.kill(-child.pid, 'SIGKILL');
                await exit;
            } finally {
                clearTimeout(timer);
            }
        }
        try {
            process.kill(-child.pid, 'SIGKILL');
        } catch (error) {
            if (error.code !== 'ESRCH') throw error;
        }
        await eventually(async () => !(await listening()), 'closed HASS port');
        this.host = null;
    }

    async newPage() {
        const page = await this.context.newPage();
        page.setDefaultTimeout(30000);
        page.on('console', message => this.log.push({ type: message.type(), text: message.text() }));
        page.on('pageerror', error => this.log.push({ type: 'pageerror', text: error.message }));
        page.on('crash', () => this.log.push({ type: 'pageerror', text: 'Browser page crashed' }));
        await page.bringToFront();
        await page.goto(this.application ? origin : `${origin}api/capabilities`, { waitUntil: 'domcontentloaded' });
        if (this.application) {
            await this.ready(page);
            await page.screenshot({ path: join(this.output, `initial-${this.context.pages().length}.png`) });
        }
        return page;
    }

    async ready(page) {
        await page.bringToFront();
        await page.waitForFunction(
            () => !!window.garminAutomation || document.querySelector('#loading')?.dataset.state === 'failure',
            null,
            { polling: 50 },
        );
        assert.equal(
            await page.locator('#loading[data-state="failure"]').count(),
            0,
            await page.locator('#loading-detail').textContent(),
        );
        await page.waitForFunction(
            () => ['ready', 'failed'].includes(window.garminMapComposition?.status().state),
            null,
            { polling: 50 },
        );
        const renderer = await page.evaluate(() => window.garminMapComposition.status());
        assert.equal(renderer.state, 'ready', JSON.stringify(renderer));
    }

    async source() {
        await openRoute(this.page, 'Neighborhood walk');
        const file = await download(this.page, 'routes.source', this.work);
        const [route] = this.storage.routes('Neighborhood walk');
        assert.deepEqual(await readFile(file), this.storage.artifact(route.source_artifact_id));
        await run(this.page, [click('routes.back'), wait('routes.import')]);
        return file;
    }

    async dispose() {
        const errors = [];
        const attempt = async action => {
            try {
                await action();
            } catch (error) {
                errors.push(error);
            }
        };
        if (this.context) {
            for (const [index, page] of this.context.pages().entries()) {
                await attempt(async () => {
                    if (!page.isClosed()) {
                        await page.bringToFront();
                        if (this.application) {
                            const state = await page.waitForFunction(
                                () => {
                                    const status = window.garminMapComposition?.status();
                                    return ['ready', 'failed'].includes(status?.state) && status;
                                },
                                null,
                                { timeout: 15000, polling: 50 },
                            );
                            this.log.push({
                                type: 'renderer',
                                page: index,
                                state: await state.jsonValue(),
                            });
                            await state.dispose();
                            await page.screenshot({ path: join(this.output, `final-${index}.png`), timeout: 5000 });
                        }
                    }
                });
            }
            if (this.tracing) await attempt(() => this.context.tracing.stop({ path: join(this.output, 'trace.zip') }));
        }
        if (this.output)
            await attempt(() => writeFile(join(this.output, 'browser.json'), JSON.stringify(this.log, null, 2)));
        await attempt(async () =>
            assert.deepEqual(
                this.log.filter(item => item.type === 'pageerror'),
                [],
                'Uncaught browser errors',
            ),
        );
        await attempt(async () => {
            for (const entry of this.log.filter(item => item.type === 'renderer')) {
                assert.equal(entry.state.state, 'ready', JSON.stringify(entry));
            }
        });
        if (this.browser) await attempt(() => this.browser.close());
        if (this.server)
            await attempt(async () => {
                try {
                    await this.server.close();
                } finally {
                    if (this.server.process().exitCode === null) await this.server.kill();
                }
            });
        await attempt(() => this.stopHost());
        for (const child of [...this.children, this.server?.process()].filter(Boolean)) {
            if (child.pid)
                await attempt(() => eventually(() => !groupExists(child.pid), `process group ${child.pid} stopped`));
        }
        if (this.work) await attempt(() => rm(this.work, { recursive: true, force: true }));
        await attempt(async () => assert.equal(await listening(), false, 'HASS port survived teardown'));
        if (errors.length)
            throw new AggregateError(
                errors,
                `Integration teardown failed: ${errors.map(error => error.message).join('; ')}`,
            );
    }
}

export async function fixture(t, options) {
    const instance = new Fixture(t.name.replace(/[^a-z0-9]+/gi, '-'), options);
    t.after(() => instance.dispose());
    await instance.start();
    return instance;
}
