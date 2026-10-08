import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import {
    click,
    courseVisible,
    download,
    eventually,
    fixture,
    openRoute,
    prepareGeneration,
    review,
    run,
    scroll,
    selectProfile,
    switchProfile,
    targets,
    upload,
    wait,
} from './fixture.mjs';

const options = { timeout: 180000 };

async function save(f, name) {
    await run(f.page, [click('routes.save'), wait('routes.title'), wait('routes.back')]);
    assert.equal((await targets(f.page)).find(t => t.id === 'routes.title').value, name);
    const routes = f.storage.routes(name);
    assert.equal(routes.length, 1);
    assert.equal(routes[0].sport, 'walking');
    return routes[0];
}

async function generate(f, name) {
    await prepareGeneration(f.page);
    await run(f.page, [click('routes.generate')]);
    await eventually(() => f.storage.courses(name).length === 1, 'one persisted Course');
    const [course] = f.storage.courses(name);
    await courseVisible(f.page, course);
    return course;
}

test('seeded routes, reviewed import, byte-exact exports and versioned deletion', options, async t => {
    const f = await fixture(t);
    const p = f.page;
    assert.deepEqual(
        f.storage.rows('SELECT sport FROM route_plan_revisions ORDER BY sport').map(row => row.sport),
        ['cycling', 'hiking', 'walking'],
    );
    assert.equal(
        f.storage.rows("SELECT count(*) AS n FROM activity_projections WHERE sport IN ('walking', 'hiking')")[0].n,
        2,
    );
    const source = await f.source();
    await review(p, source, 'Imported walk');
    assert((await targets(p)).some(t => t.id === 'map'));
    const route = await save(f, 'Imported walk');
    const original = await download(p, 'routes.source', f.work);
    assert.deepEqual(await readFile(original), await readFile(source));
    assert.deepEqual(await readFile(original), f.storage.artifact(route.source_artifact_id));
    const first = await generate(f, 'Imported walk');
    assert.equal(first.version, 1);
    const fit = await download(p, `routes.download.${first.id}`, f.work);
    const bytes = await readFile(fit);
    assert.deepEqual(bytes, f.storage.artifact(first.artifact_id));
    assert.equal(bytes.subarray(8, 12).toString(), '.FIT');
    await run(p, [click(`routes.delete.${first.id}`), wait('routes.delete.cancel'), click('routes.delete.cancel')]);
    assert.deepEqual(f.storage.courses('Imported walk'), [first]);
    await run(p, [
        click(`routes.delete.${first.id}`),
        wait('routes.delete.confirm'),
        click('routes.delete.confirm'),
        wait('routes.title'),
    ]);
    await eventually(() => f.storage.courses('Imported walk').length === 0, 'confirmed Course deletion');
    const second = await generate(f, 'Imported walk');
    assert.equal(second.version, 2);
    assert.notEqual(second.id, first.id);
    const baseline = f.storage.snapshot();
    await p.reload({ waitUntil: 'domcontentloaded' });
    await f.ready(p);
    await p.screenshot({ path: join(f.output, 'reload.png') });
    await selectProfile(p);
    await openRoute(p, 'Imported walk');
    await courseVisible(p, second);
    assert.deepEqual(f.storage.snapshot(), baseline);
    await switchProfile(p, 1);
    assert.equal((await targets(p)).filter(t => t.id.startsWith('routes.open.')).length, 0);
    await switchProfile(p, 0);
    await review(p, source, 'Imported walk');
    await run(p, [click('routes.save'), wait('routes.title')]);
    const duplicates = f.storage.routes('Imported walk');
    assert.equal(duplicates.length, 2, 'Explicit duplicate names remain separate routes');
    assert.notEqual(duplicates[0].plan_id, duplicates[1].plan_id);
});

test('narrow import labels and controls stay inside the viewport', options, async t => {
    const f = await fixture(t);
    const p = f.page;
    const source = await f.source();
    await p.setViewportSize({ width: 390, height: 844 });
    await p.mouse.click(16, 16);
    await run(p, [wait('routes.import')]);
    await upload(p, source);
    const candidate = 'routes.candidate.TrackSegment { track: 0, segment: 0 }';
    await run(p, [wait(candidate), click(candidate), wait('routes.name')]);
    const ids = [
        'routes.name',
        'routes.name.label',
        'routes.type.label',
        'routes.sport.walking',
        'routes.sport.hiking',
        'routes.sport.running',
        'routes.sport.cycling',
    ];
    await p.waitForFunction(
        ids =>
            ids.every(id => {
                const bounds = window.garminAutomation.targets().find(target => target.id === id)?.bounds;
                return bounds && bounds[0] >= 0 && bounds[2] <= window.innerWidth;
            }),
        ids,
        { polling: 50 },
    );
    const controls = await targets(p);
    for (const id of ids) {
        const bounds = controls.find(t => t.id === id)?.bounds;
        assert(bounds, `Missing narrow control: ${id}`);
        assert(bounds[0] >= 0 && bounds[2] <= 390, `Clipped narrow control: ${id}`);
    }
    assert(
        controls.find(t => t.id === 'routes.type.label').bounds[1] >=
            controls.find(t => t.id === 'routes.name').bounds[3],
    );
    assert.equal(controls.find(t => t.id === 'routes.save').enabled, false);
});

test('invalid GPX siblings, unresolved controls and malformed upload recovery', options, async t => {
    const f = await fixture(t);
    const p = f.page;
    await upload(p, process.env.GARMIN_GPX_CANDIDATES);
    const control = 'routes.candidate.Route { route: 0 }';
    await run(p, [wait(control)]);
    const candidates = (await targets(p)).filter(t => t.id.startsWith('routes.candidate.'));
    assert.equal(candidates.length, 3);
    assert.equal(candidates.filter(t => t.label.startsWith('Morning loop')).length, 2);
    assert(!candidates.some(t => t.id.includes('track: 1')));
    await run(p, [
        click(control),
        wait('routes.sport.hiking'),
        click('routes.sport.hiking'),
        wait('routes.save'),
        click('routes.save'),
        wait('routes.title'),
        wait('routes.back'),
        scroll('routes.title', -1200),
    ]);
    assert.equal((await targets(p)).find(t => t.id === 'routes.generate').enabled, false);
    assert.equal(f.storage.routes('Control points')[0].sport, 'hiking');
    await run(p, [click('routes.back'), wait('routes.import')]);
    const malformed = join(f.work, 'malformed.gpx');
    await writeFile(malformed, '<gpx><trk></gpx>');
    const before = f.storage.snapshot();
    await upload(p, malformed);
    await run(p, [wait('routes.retry'), click('routes.cancel'), wait('routes.import')]);
    assert.deepEqual(f.storage.snapshot(), before);
    const source = await f.source();
    await review(p, source, 'After rejection');
    await save(f, 'After rejection');
});

for (const interruption of ['profile switch', 'disconnect']) {
    test(`upload interrupted by ${interruption} leaves no partial route`, options, async t => {
        const f = await fixture(t, { faults: true });
        const p = f.page;
        const source = await f.source();
        const before = f.storage.snapshot();
        await p.evaluate(() => (window.integrationFault.holdReads = true));
        await upload(p, source);
        await p.waitForFunction(() => window.integrationFault.reads.length > 0, null, { polling: 50 });
        if (interruption === 'profile switch') {
            await switchProfile(p, 1);
            await p.evaluate(() => window.integrationFault.release('reads'));
            await run(p, [wait('routes.import')]);
            assert.equal((await targets(p)).filter(t => t.id.startsWith('routes.open.')).length, 0);
            await switchProfile(p, 0);
        } else {
            await p.evaluate(() => window.integrationFault.disconnect());
            await run(p, [wait('routes.import')]);
        }
        assert.deepEqual(f.storage.snapshot(), before);
        await review(p, source, 'After interruption');
        await save(f, 'After interruption');
    });
}

test('lost committed import and Course replies retry without duplicates', options, async t => {
    const f = await fixture(t, { faults: true });
    const p = f.page;
    const source = await f.source();
    const name = 'Lost response';
    await review(p, source, name);
    await p.evaluate(() => (window.integrationFault.holdIncoming = true));
    await run(p, [click('routes.save')]);
    await eventually(() => f.storage.routes(name).length === 1, 'import committed before response');
    const [route] = f.storage.routes(name);
    assert((await p.evaluate(() => window.integrationFault.incoming.length)) > 0);
    await p.evaluate(() => window.integrationFault.disconnect());
    await run(p, [
        wait('routes.name'),
        scroll('routes.name', 1600),
        wait('routes.retry'),
        click('routes.retry'),
        wait('routes.title'),
    ]);
    assert.deepEqual(f.storage.routes(name), [route]);
    await prepareGeneration(p);
    await p.evaluate(() => (window.integrationFault.holdIncoming = true));
    await run(p, [click('routes.generate')]);
    // Let preparation replies through, stopping as soon as the independent storage oracle sees the commit.
    await eventually(async () => {
        if (f.storage.courses(name).length) return true;
        await p.evaluate(() => {
            for (const resume of window.integrationFault.incoming.splice(0)) resume();
        });
        return false;
    }, 'Course committed with its response withheld');
    const [course] = f.storage.courses(name);
    assert.equal(course.version, 1);
    assert(!(await targets(p)).some(t => t.id === `routes.download.${course.id}`));
    await p.evaluate(() => window.integrationFault.disconnect());
    await p.mouse.move(800, 400);
    await p.mouse.wheel(0, -1600);
    await run(p, [wait('routes.retry'), click('routes.retry')]);
    await courseVisible(p, course);
    assert.deepEqual(f.storage.courses(name), [course]);
    assert.equal(f.storage.counts().routes, 4);
});

for (const pending of ['confirmation', 'generation', 'download']) {
    test(`restore invalidates old client with queued ${pending}`, options, async t => {
        const f = await fixture(t, { faults: true });
        const p = f.page;
        const source = await f.source();
        await review(p, source, 'Snapshot course');
        await save(f, 'Snapshot course');
        const course = await generate(f, 'Snapshot course');
        const baseline = f.storage.snapshot();
        const backupPage = await f.newPage();
        await run(backupPage, [
            wait('profile.0'),
            click('profile.0'),
            wait('profile.toggle'),
            click('profile.toggle'),
            wait('profile.backup'),
            click('profile.backup'),
            wait('backup.save'),
        ]);
        const archive = await download(backupPage, 'backup.save', f.work);
        await run(backupPage, [wait('backup.clear'), click('backup.clear'), wait('backup.open')]);
        await upload(backupPage, archive, 'backup.open');
        await run(backupPage, [wait('backup.approve')]);
        await p.bringToFront();
        let action;
        if (pending === 'confirmation') {
            await run(p, [click('routes.back'), wait('routes.import')]);
            await review(p, source, 'Stale confirmation');
            action = 'routes.save';
        } else if (pending === 'generation') {
            await run(p, [click('routes.back'), wait('routes.import')]);
            await openRoute(p, 'Neighborhood walk');
            await prepareGeneration(p);
            action = 'routes.generate';
        } else {
            action = `routes.download.${course.id}`;
        }
        let staleDownloads = 0;
        p.on('download', () => staleDownloads++);
        await p.evaluate(() => (window.integrationFault.holdOutgoing = true));
        await run(p, [click(action)]);
        await p.waitForFunction(() => window.integrationFault.outgoing.length > 0, null, { polling: 50 });
        const oldPath = f.storage.path();
        await run(backupPage, [click('backup.approve'), wait('backup.restore.acknowledge')]);
        assert.notEqual(f.storage.path(), oldPath);
        await p.evaluate(() => window.integrationFault.release('outgoing'));
        await p.bringToFront();
        await selectProfile(p);
        assert.deepEqual(f.storage.snapshot(), baseline);
        assert.equal(staleDownloads, 0);
        await openRoute(p, 'Snapshot course');
        await courseVisible(p, course);
        const restored = await download(p, `routes.download.${course.id}`, f.work);
        assert.deepEqual(await readFile(restored), f.storage.artifact(course.artifact_id));
        await f.stopHost();
        await f.startHost();
        await selectProfile(p);
        assert.deepEqual(f.storage.snapshot(), baseline);
    });
}

test('abrupt host exit preserves committed data and does not reseed duplicates', options, async t => {
    const f = await fixture(t);
    const p = f.page;
    const source = await f.source();
    await review(p, source, 'Survives restart');
    await save(f, 'Survives restart');
    const course = await generate(f, 'Survives restart');
    const baseline = f.storage.snapshot();
    for (const signal of ['SIGKILL', 'SIGTERM']) {
        await f.stopHost(signal);
        await f.startHost();
        await selectProfile(p);
        assert.deepEqual(f.storage.snapshot(), baseline);
        await openRoute(p, 'Survives restart');
        await courseVisible(p, course);
        const file = await download(p, `routes.download.${course.id}`, f.work);
        assert.deepEqual(await readFile(file), f.storage.artifact(course.artifact_id));
    }
});
