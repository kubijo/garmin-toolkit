import assert from 'node:assert/strict';
import test from 'node:test';
import { fixture, selectProfile } from './fixture.mjs';

for (const application of [false, true]) {
    for (const transport of ['stream', 'standard']) {
        for (const faults of [false, true]) {
            for (const tracing of [false, true]) {
                const subject = application
                    ? `application ${transport === 'stream' ? 'stream-available' : 'standard-only'}`
                    : `socket ${transport}`;
                const name = `reconnect ${subject} faults=${faults} tracing=${tracing}`;
                test(name, { timeout: 180000 }, async t => {
                    const f = await fixture(t, { application, transport, faults, tracing });
                    const p = f.page;
                    assert.equal(await p.evaluate(() => 'WebSocketStream' in window), transport === 'stream');
                    assert.equal(await p.evaluate(() => !!window.integrationFault), faults);
                    const baseline = f.storage.snapshot();
                    for (let cycle = 0; cycle < 8; cycle++) {
                        if (!application) {
                            await p.evaluate(async transport => {
                                const url = `ws://${location.host}/remoc`;
                                const state = { received: 0, closed: false, drained: false };
                                window.socketProbe = state;
                                if (transport === 'stream') {
                                    const socket = new WebSocketStream(url);
                                    socket.closed
                                        .catch(() => {})
                                        .finally(() => {
                                            state.closed = true;
                                        });
                                    const { readable } = await socket.opened;
                                    const reader = readable.getReader();
                                    state.pump = (async () => {
                                        try {
                                            while (!(await reader.read()).done) state.received++;
                                        } catch {
                                            // An abrupt server exit rejects pending reads.
                                        } finally {
                                            reader.releaseLock();
                                            socket.close();
                                            state.drained = true;
                                        }
                                    })();
                                } else {
                                    const socket = new WebSocket(url);
                                    socket.onmessage = () => state.received++;
                                    socket.onclose = () => {
                                        state.closed = true;
                                        state.drained = true;
                                    };
                                    await new Promise((resolve, reject) => {
                                        socket.onopen = resolve;
                                        socket.onerror = reject;
                                    });
                                }
                            }, transport);
                            await p.waitForFunction(() => window.socketProbe.received > 0, null, { polling: 50 });
                        }
                        await f.stopHost(cycle % 2 ? 'SIGTERM' : 'SIGKILL');
                        if (!application) {
                            await p.waitForFunction(
                                () => window.socketProbe.closed && window.socketProbe.drained,
                                null,
                                {
                                    polling: 50,
                                },
                            );
                        }
                        await f.startHost();
                        if (application) await selectProfile(p);
                        assert.deepEqual(f.storage.snapshot(), baseline);
                        t.diagnostic(`Completed reconnect ${cycle + 1}/8`);
                    }
                    const connections = await p.evaluate(() => window.integrationSockets);
                    if (application) {
                        assert.equal(connections.stream, 0, 'Application must not select the experimental stream API');
                        assert(connections.standard >= 9, JSON.stringify(connections));
                    } else {
                        assert.equal(connections[transport], 8);
                    }
                });
            }
        }
    }
}
