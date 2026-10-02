import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../../crates/garmin-diagnostics/view.js', import.meta.url), 'utf8');

function fixture() {
    const records = [];
    const elements = Object.fromEntries(
        ['connection', 'history', 'state', 'status'].map(id => [
            id,
            {
                textContent: '',
                childElementCount: 0,
                insertAdjacentHTML: (_, html) => records.push(html),
            },
        ]),
    );
    const lifecycle = new EventTarget();
    const streams = [];
    class EventSource {
        constructor(url) {
            this.url = new URL(url);
            this.listeners = new Map();
            this.closed = false;
            streams.push(this);
        }
        addEventListener(name, callback) {
            this.listeners.set(name, callback);
        }
        close() {
            this.closed = true;
        }
        update(cursor) {
            this.listeners.get('view')({
                lastEventId: cursor,
                data: JSON.stringify({ state: '', records: cursor, status: cursor }),
            });
        }
    }
    runInNewContext(source, {
        URL,
        EventSource,
        document: {
            getElementById: id => elements[id],
            body: { dataset: { cursor: 'epoch:1' } },
        },
        location: { href: 'http://localhost:8099/api/events-stream?kind=window&limit=5' },
        addEventListener: lifecycle.addEventListener.bind(lifecycle),
    });
    return {
        streams,
        elements,
        records,
        dispatch(name, persisted = true) {
            const event = new Event(name);
            Object.defineProperty(event, 'persisted', { value: persisted });
            lifecycle.dispatchEvent(event);
        },
    };
}

test('restoring the live page resumes its latest cursor on every Back/Forward visit', () => {
    const page = fixture();
    assert.equal(page.streams[0].url.searchParams.get('after'), 'epoch:1');
    page.dispatch('pageshow', false);
    assert.equal(page.streams.length, 1);
    for (let visit = 0; visit < 3; visit++) {
        const stream = page.streams.at(-1);
        const cursor = `epoch:${visit + 2}`;
        stream.update(cursor);
        assert.equal(page.elements.connection.textContent, 'Connected');
        page.dispatch('pagehide');
        assert.equal(stream.closed, true);
        assert.notEqual(page.elements.connection.textContent, 'Connected');
        page.dispatch('pageshow');
        const restored = page.streams.at(-1);
        assert.equal(page.streams.length, visit + 2);
        assert.equal(restored.closed, false);
        assert.equal(restored.url.searchParams.get('after'), cursor);
        assert.equal(restored.url.searchParams.get('kind'), 'window');
        assert.equal(restored.url.searchParams.get('limit'), '5');
        assert.equal(page.elements.connection.textContent, 'Connecting…');
    }
});

test('callbacks from closed streams cannot change the restored view or resume cursor', () => {
    const page = fixture();
    const old = page.streams[0];
    old.update('epoch:2');
    page.dispatch('pagehide');
    page.dispatch('pageshow');
    const current = page.streams.at(-1);
    current.update('epoch:3');
    old.update('epoch:99');
    old.onerror();
    assert.equal(page.elements.connection.textContent, 'Connected');
    assert.deepEqual(page.records, ['epoch:2', 'epoch:3']);
    current.onerror();
    assert.equal(page.elements.connection.textContent, 'Disconnected. Reconnecting…');
    page.dispatch('pagehide');
    page.dispatch('pageshow');
    assert.equal(page.streams.at(-1).url.searchParams.get('after'), 'epoch:3');
});
