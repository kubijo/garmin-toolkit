// Drive the packaged app with its existing browser-local semantic API.
import assert from 'node:assert/strict';
import { readFile, readdir, rename, stat, writeFile, mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

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

export async function populatedWorkflow(client, state, sessionPath, evidencePath) {
    const runtime = dirname(sessionPath);
    const downloads = resolve(runtime, 'downloads');
    const source = resolve('crates/garmin-gpx/tests/fixtures/candidates.gpx');
    const ingress = new URL('/api/hassio_ingress/', state.base_url).href;
    await client.call('Runtime.enable');
    async function app(fn, argument) {
        function find(tree) {
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
        return client.evaluate(fn, argument, context.id);
    }
    await mkdir(downloads);
    // Unique download GUIDs distinguish repeated downloads of the same artifact.
    await client.call('Browser.setDownloadBehavior', { behavior: 'allowAndName', downloadPath: downloads });
    await client.call('Page.setInterceptFileChooserDialog', { enabled: true });
    await client.call('Page.addScriptToEvaluateOnNewDocument', {
        source: `(${enableBrowserAutomation.toString()})()`,
    });

    async function until(check, description, timeout = 45_000) {
        const deadline = Date.now() + timeout;
        let last;
        while (Date.now() < deadline) {
            assert.equal(client.socket.readyState, WebSocket.OPEN, 'the owned browser is connected');
            try {
                const result = await check();
                if (result) return result;
            } catch (error) {
                last = error;
            }
            await sleep(100);
        }
        throw Error(`Timed out waiting for ${description}: ${last ?? 'not ready'}`);
    }

    const targets = () => app(() => window.garminAutomation?.targets());
    const target = id => until(async () => (await targets())?.find(node => node.id === id), id);
    const click = id => ({ kind: 'click', target: id });
    const wait = id => ({ kind: 'wait', target: id });
    const scroll = (id, delta) => ({ kind: 'scroll', target: id, delta });

    async function run(actions) {
        await app(actions => window.garminAutomation.sequence(actions, { run_in_background: true }), actions);
        const report = await until(async () => {
            const report = await app(() => window.garminAutomation.status());
            return report && !['running', 'paused'].includes(report.state) ? report : null;
        }, 'semantic actions');
        assert.equal(report.state, 'passed', JSON.stringify({ actions, report, targets: await targets() }));
        assert.equal(report.completed, report.total);
        await until(
            async () => !(await app(() => window.garminAutomation.status())).needs_background_frames,
            'semantic input release',
        );
    }

    async function capture(name) {
        const result = await until(
            () =>
                app(async () => {
                    const [info, bytes] = await window.garminWindowControl.capture(5000);
                    return { info: JSON.parse(info), png: bytes.toBase64() };
                }),
            `stable app-owned ${name} screenshot`,
        );
        const png = Buffer.from(result.png, 'base64');
        assert.ok(png.length > 100 && png.length <= 8 * 1024 * 1024);
        await writeFile(resolve(evidencePath, `${name}.png`), png);
        await writeFile(resolve(evidencePath, `${name}.json`), JSON.stringify(result.info, null, 2));
    }

    async function createProfile(name) {
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
        await client.call('DOM.setFileInputFiles', { backendNodeId: chooser.params.backendNodeId, files: [source] });
    }

    async function download(id, filename) {
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

    async function courseTarget() {
        return until(
            async () => (await targets())?.find(node => node.id.startsWith('routes.download.')),
            'generated FIT Course',
        );
    }

    async function exports(prefix) {
        const course = await courseTarget();
        await capture(`${prefix}-course`);
        await download(course.id, `${prefix}-course.fit`);
        await run([scroll(course.id, 1600)]);
        await download('routes.source', `${prefix}-source.gpx`);
        await run([scroll('routes.source', -1600)]);
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
        await until(async () => !(await targets()).some(node => node.id === course.id), 'removed FIT Course');
        await run([click('profile.toggle'), wait('profile.logout'), click('profile.logout')]);
        await createProfile('After backup');
        await capture('mutated-profile');
    }

    let lastId = 0;
    while (true) {
        const request = await until(
            async () => {
                try {
                    const value = JSON.parse(await readFile(resolve(runtime, 'workflow-request.json'), 'utf8'));
                    return value.id > lastId ? value : null;
                } catch (error) {
                    if (error.code === 'ENOENT') return null;
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
            else if (['restarted', 'restored'].includes(request.operation)) {
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
