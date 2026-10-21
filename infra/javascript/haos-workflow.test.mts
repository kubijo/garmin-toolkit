import assert from 'node:assert/strict';
import test from 'node:test';
import { findCourseTarget, findVisibleTarget } from './haos-workflow.mts';
import type { Action, Target, Until } from './haos-types.mts';

const until: Until = async (check, description) => {
    for (let attempt = 0; attempt < 5; attempt++) {
        try {
            const result = await check();
            if (result) return result as NonNullable<Exclude<typeof result, false | 0 | ''>>;
        } catch {
            // Match the browser's polling of transient execution-context failures.
        }
    }
    throw Error(`Timed out waiting for ${description}`);
};
const history: Target = {
    id: 'routes.history',
    label: 'Previous FIT files',
    enabled: true,
    bounds: [256, 869.25, 388, 873],
};
const course: Target = {
    id: 'routes.download.original',
    label: 'Download FIT',
    enabled: true,
    bounds: [800, 700, 840, 740],
};

test('current Course remains selected when history is also available', async () => {
    const actions: Action[] = [];
    const result = await findCourseTarget(
        async () => [history, course],
        async sequence => {
            actions.push(...sequence);
        },
        until,
    );
    assert.equal(result.id, course.id);
    assert.deepEqual(actions, [], 'a visible Course needs no history interaction');
});

test('published-baseline Course is opened and scrolled into view without regeneration', async () => {
    let open = false;
    let scrolled = false;
    let frames = 0;
    const actions: Action[] = [];
    const result = await findCourseTarget(
        async () => (open ? [history, scrolled && frames++ >= 2 ? course : { ...course, bounds: null }] : [history]),
        async sequence => {
            for (const action of sequence) {
                actions.push(action);
                assert.equal(action.target, history.id, 'only the existing history control is used');
                if (action.kind === 'click') open = !open;
                else {
                    assert.ok(action.kind === 'scroll');
                    assert.ok(open, 'history must be expanded before scrolling');
                    assert.ok(action.delta < 0, 'scroll toward the retained Course');
                    scrolled = true;
                }
            }
        },
        until,
    );
    assert.equal(result.id, course.id);
    assert.equal(actions.filter(action => action.kind === 'click').length, 1, 'polling must not close history');
});

test('an expanded history entry must be scrolled into view before it can be used', async () => {
    let scrolled = false;
    const result = await findCourseTarget(
        async () => [history, scrolled ? course : { ...course, bounds: null }],
        async actions => {
            for (const action of actions) {
                assert.ok(action.kind === 'scroll', 'expanded history must not be toggled closed');
                assert.equal(action.target, history.id, 'scroll from a visible control');
                scrolled = true;
            }
        },
        until,
    );
    assert.notEqual(result.bounds, null, 'discovering an off-screen Course does not make it usable');
    assert.equal(scrolled, true);
});

const source: Target = { id: 'routes.source', label: 'Download GPX', enabled: true, bounds: [800, 56, 1000, 96] };

test('returning to GPX after a download does not scroll from the now-clipped Course', async () => {
    const actions: Action[] = [];
    const result = await findVisibleTarget(
        async () => [{ ...course, bounds: null }, source],
        async sequence => {
            actions.push(...sequence);
        },
        until,
        source.id,
        1600,
    );
    assert.equal(result.id, source.id);
    assert.deepEqual(actions, [], 'the GPX control is already visible after the layout changes');
});

test('a failed semantic scroll remains fatal even if its target subsequently becomes visible', async () => {
    const failure = Error('semantic scroll failed');
    let reads = 0;
    await assert.rejects(
        findVisibleTarget(
            async () => [course, ++reads > 1 ? source : { ...source, bounds: null }],
            async () => {
                throw failure;
            },
            until,
            source.id,
            1600,
        ),
        error => error === failure,
    );
});

test('a clipped GPX control is revealed from a visible Course control', async () => {
    let scrolled = false;
    const result = await findVisibleTarget(
        async () => [course, scrolled ? source : { ...source, bounds: null }],
        async actions => {
            for (const action of actions) {
                assert.ok(action.kind === 'scroll' && action.delta > 0);
                assert.equal(action.target, course.id);
                scrolled = true;
            }
        },
        until,
        source.id,
        1600,
    );
    assert.equal(scrolled, true);
    assert.notEqual(result.bounds, null);
});

test('a permanently clipped or disabled Course remains fatal', async () => {
    for (const unavailable of [
        { ...course, bounds: null },
        { ...course, enabled: false },
    ]) {
        await assert.rejects(
            findCourseTarget(
                async () => [unavailable],
                async () => assert.fail('there is no usable scroll anchor'),
                until,
            ),
            /Timed out waiting for visible routes.download.original/,
        );
    }
});

test('a missing Course remains fatal even when history can be opened', async () => {
    for (const targets of [[], [history]]) {
        await assert.rejects(
            findCourseTarget(
                async () => targets,
                async () => {},
                until,
            ),
            /Timed out waiting for/,
        );
    }
});
