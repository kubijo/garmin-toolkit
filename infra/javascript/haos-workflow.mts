// Drive the packaged app with its existing browser-local semantic API.
import assert from 'node:assert/strict';
import { readFile, readdir, rename, stat, writeFile, mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import type { Protocol } from 'devtools-protocol';
import type { DevTools } from './haos-devtools.mts';
import type { Action, BrowserFunction, ReadySession, Target, Until } from './haos-types.mts';

export async function findVisibleTarget(
    targets: () => Promise<Target[] | undefined>,
    run: (actions: Action[]) => Promise<void>,
    until: Until,
    id: string,
    delta: number,
): Promise<Target> {
    const result = await until(async () => {
        const current = await targets();
        const target = current?.find(node => node.id === id);
        if (target?.enabled && target.bounds) return { kind: 'ready' as const, target };
        const anchor = current?.find(node => node.id.startsWith('routes.') && node.enabled && node.bounds);
        if (anchor) {
            try {
                await run([{ kind: 'scroll', target: anchor.id, delta }]);
            } catch (error) {
                // Polling retries transient reads; a failed action must still abort.
                return { kind: 'failed' as const, error };
            }
        }
        return undefined;
    }, `visible ${id}`);
    if (result.kind === 'failed') throw result.error;
    return result.target;
}

export async function findCourseTarget(
    targets: () => Promise<Target[] | undefined>,
    run: (actions: Action[]) => Promise<void>,
    until: Until,
): Promise<Target> {
    let course = await until(async () => {
        const current = await targets();
        return (
            current?.find(node => node.id.startsWith('routes.download.')) ??
            current?.find(node => node.id === 'routes.history')
        );
    }, 'generated FIT Course or previous FIT files');
    if (course.id === 'routes.history') {
        // An encoder version change moves the original Course into collapsed history.
        // Open it once, then wait for the retained file to enter the semantic tree.
        await findVisibleTarget(targets, run, until, course.id, -1600);
        await run([{ kind: 'click', target: course.id }]);
        course = await until(
            async () => (await targets())?.find(node => node.id.startsWith('routes.download.')),
            'stored FIT Course in previous FIT files',
        );
    }
    return findVisibleTarget(targets, run, until, course.id, -1600);
}

function enableBrowserAutomation() {
    // This changes only this disposable browser's DOM. The production host keeps
    // its HTTP control routes disabled, and the served bundle is not rewritten.
    const observer = new MutationObserver(() => {
        const canvas = document.querySelector('#garmin-toolkit');
        if (!canvas) return;
        canvas.setAttribute('data-ui-automation', 'true');
        observer.disconnect();
    });
    observer.observe(document, { childList: true, subtree: true });
}

export async function populatedWorkflow(
    client: DevTools,
    state: ReadySession,
    sessionPath: string,
    evidencePath: string,
) {
    const runtime = dirname(sessionPath);
    const downloads = resolve(runtime, 'downloads');
    const source = resolve('crates/garmin-gpx/tests/fixtures/candidates.gpx');
    const ingress = new URL('/api/hassio_ingress/', state.base_url).href;
    await client.call('Runtime.enable');
    function app<R>(fn: () => R): Promise<Awaited<R>>;
    function app<A, R>(fn: BrowserFunction<A, R>, argument: A): Promise<Awaited<R>>;
    async function app(fn: { toString(): string }, argument?: unknown): Promise<unknown> {
        function find(tree: Protocol.Page.FrameTree): Protocol.Page.Frame | null {
            if (tree.frame.url.startsWith(ingress)) return tree.frame;
            for (const child of tree.childFrames ?? []) {
                const frame = find(child);
                if (frame) return frame;
            }
            return null;
        }
        const tree = await client.call('Page.getFrameTree');
        const frame = find(tree.frameTree);
        assert.ok(frame, 'the owned ingress iframe is available');
        const context = [...client.contexts.values()].find(
            context => context.auxData?.isDefault && context.auxData.frameId === frame.id,
        );
        assert.ok(context, 'the ingress execution context is available');
        return client.evaluate(fn as BrowserFunction<unknown, unknown>, argument, context.id);
    }
    await mkdir(downloads);
    // Unique download GUIDs distinguish repeated downloads of the same artifact.
    await client.call('Browser.setDownloadBehavior', { behavior: 'allowAndName', downloadPath: downloads });
    await client.call('Page.setInterceptFileChooserDialog', { enabled: true });
    await client.call('Page.addScriptToEvaluateOnNewDocument', {
        source: `(${enableBrowserAutomation.toString()})()`,
    });

    async function until<T>(
        check: () => T | Promise<T>,
        description: string,
        timeout = 45_000,
    ): Promise<NonNullable<Exclude<T, false | 0 | ''>>> {
        const deadline = Date.now() + timeout;
        let last: unknown;
        while (Date.now() < deadline) {
            assert.equal(client.socket.readyState, WebSocket.OPEN, 'the owned browser is connected');
            try {
                const result = await check();
                if (result) return result as NonNullable<Exclude<T, false | 0 | ''>>;
            } catch (error) {
                last = error;
            }
            await sleep(100);
        }
        throw Error(`Timed out waiting for ${description}: ${last ?? 'not ready'}`);
    }

    const targets = () => app(() => window.garminAutomation?.targets());
    const target = (id: string) => until(async () => (await targets())?.find(node => node.id === id), id);
    const click = (id: string): Action => ({ kind: 'click', target: id });
    const wait = (id: string): Action => ({ kind: 'wait', target: id });
    const scroll = (id: string, delta: number): Action => ({ kind: 'scroll', target: id, delta });

    async function run(actions: Action[]) {
        await app(actions => {
            if (!window.garminAutomation) throw Error('semantic automation is unavailable');
            return window.garminAutomation.sequence(actions, { run_in_background: true });
        }, actions);
        const report = await until(async () => {
            const report = await app(() => window.garminAutomation?.status());
            return report && !['running', 'paused'].includes(report.state) ? report : null;
        }, 'semantic actions');
        assert.equal(report.state, 'passed', JSON.stringify({ actions, report, targets: await targets() }));
        assert.equal(report.completed, report.total);
        await until(async () => {
            const report = await app(() => window.garminAutomation?.status());
            return report && !report.needs_background_frames;
        }, 'semantic input release');
    }

    async function capture(name: string) {
        const result = await until(
            () =>
                app(async () => {
                    if (!window.garminWindowControl) throw Error('app capture is unavailable');
                    const [info, bytes] = await window.garminWindowControl.capture(5000);
                    return { info: JSON.parse(info) as unknown, png: bytes.toBase64() };
                }),
            `stable app-owned ${name} screenshot`,
        );
        const png = Buffer.from(result.png, 'base64');
        assert.ok(png.length > 100 && png.length <= 8 * 1024 * 1024);
        await writeFile(resolve(evidencePath, `${name}.png`), png);
        await writeFile(resolve(evidencePath, `${name}.json`), JSON.stringify(result.info, null, 2));
    }

    async function createProfile(name: string) {
        await run([wait('profile.create'), click('profile.create'), wait('profile.create.name')]);
        await run([{ kind: 'text', target: 'profile.create.name', text: name }, click('profile.create.submit')]);
        await target('navigation.1');
    }

    async function upload() {
        client.events = client.events.filter(event => event.method !== 'Page.fileChooserOpened');
        await run([click('routes.import')]);
        const chooser = await until(
            () => client.events.find(event => event.method === 'Page.fileChooserOpened'),
            'browser file chooser',
        );
        assert.ok(chooser.params.backendNodeId, 'the file chooser belongs to an input');
        await client.call('DOM.setFileInputFiles', { backendNodeId: chooser.params.backendNodeId, files: [source] });
    }

    async function download(id: string, filename: string) {
        const existing = new Set(await readdir(downloads));
        await run([wait(id), click(id)]);
        const file = await until(async () => {
            const files = (await readdir(downloads)).filter(
                name => !existing.has(name) && !name.endsWith('.crdownload'),
            );
            assert.ok(files.length <= 1, 'exactly one browser download per action');
            return files[0];
        }, `completed ${filename} browser download`);
        const path = resolve(downloads, file);
        const metadata = await stat(path);
        assert.ok(metadata.size > 0 && metadata.size <= 8 * 1024 * 1024);
        await writeFile(resolve(evidencePath, filename), await readFile(path));
    }

    const courseTarget = () => findCourseTarget(targets, run, until);

    async function exports(prefix: string) {
        const course = await courseTarget();
        await capture(`${prefix}-course`);
        await download(course.id, `${prefix}-course.fit`);
        // Downloading briefly disables generation and changes the history layout.
        // The Course can leave the viewport; choose a currently visible scroll anchor.
        await findVisibleTarget(targets, run, until, 'routes.source', 1600);
        await download('routes.source', `${prefix}-source.gpx`);
    }

    async function openPopulated() {
        await client.call('Page.navigate', { url: state.url });
        await target('profile.0');
        await run([click('profile.0'), wait('navigation.1'), click('navigation.1')]);
        const route = await until(
            async () =>
                (await targets())?.find(
                    node => node.id.startsWith('routes.open.') && node.label.startsWith('Ingress loop ·'),
                ),
            'persisted route',
        );
        await run([click(route.id), wait('routes.title'), scroll('routes.title', -1600)]);
        assert.equal((await target('routes.title')).value, 'Ingress loop');
    }

    async function populate() {
        await client.call('Page.reload');
        await target('profile.create');
        await capture('workflow-before');
        await createProfile('Ingress Runner');
        await run([click('navigation.1'), wait('routes.import')]);
        await upload();
        const candidate = 'routes.candidate.TrackSegment { track: 0, segment: 0 }';
        await run([wait(candidate), click(candidate), wait('routes.name')]);
        await run([
            { kind: 'key', target: 'routes.name', key: 'A', modifier: 'ctrl' },
            { kind: 'text', target: 'routes.name', text: 'Ingress loop', click_first: false },
            click('routes.sport.walking'),
            scroll('routes.name', -1600),
            wait('routes.save'),
            click('routes.save'),
            wait('routes.title'),
            scroll('routes.title', -1600),
            wait('routes.generate'),
            click('routes.generate'),
        ]);
        await exports('populated');
    }

    async function mutate() {
        await openPopulated();
        const course = await courseTarget();
        const id = course.id.replace('routes.download.', 'routes.delete.');
        await run([click(id), wait('routes.delete.confirm'), click('routes.delete.confirm')]);
        await until(async () => {
            const current = await targets();
            return current && !current.some(node => node.id === course.id);
        }, 'removed FIT Course');
        await run([click('profile.toggle'), wait('profile.logout'), click('profile.logout')]);
        await createProfile('After backup');
        await capture('mutated-profile');
    }

    let lastId = 0;
    while (true) {
        const request = await until(
            async () => {
                try {
                    const value = JSON.parse(await readFile(resolve(runtime, 'workflow-request.json'), 'utf8')) as {
                        id: number;
                        operation: string;
                    };
                    return value.id > lastId ? value : null;
                } catch (error) {
                    if (typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT')
                        return null;
                    throw error;
                }
            },
            'owned workflow command',
            900_000,
        );
        lastId = request.id;
        try {
            if (request.operation === 'populate') await populate();
            else if (request.operation === 'mutate') await mutate();
            else if (request.operation === 'reinstalled') {
                await client.call('Page.navigate', { url: state.url });
                await target('profile.create');
                const current = await targets();
                assert.ok(current, 'the reinstalled app exposes semantic targets');
                assert.ok(!current.some(node => node.id === 'profile.0'), 'reinstall has no old profile');
                await capture('reinstalled-empty');
            } else if (
                [
                    'upgraded',
                    'restarted',
                    'rebooted',
                    'restored',
                    'recovered',
                    'recovered-upgraded',
                    'withdrawn',
                    'withdrawal-replaced',
                ].includes(request.operation)
            ) {
                await openPopulated();
                await exports(request.operation);
            } else throw Error(`Unknown workflow command: ${request.operation}`);
            const pending = resolve(runtime, 'workflow-response.pending');
            await writeFile(pending, JSON.stringify({ id: request.id }));
            await rename(pending, resolve(runtime, 'workflow-response.json'));
        } catch (error) {
            try {
                await capture('workflow-failed');
            } catch {
                // The outer browser runner also captures the HA page on failure.
            }
            throw error;
        }
    }
}
