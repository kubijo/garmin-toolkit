import assert from 'node:assert/strict';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { runInNewContext } from 'node:vm';
import { installFaults } from './faults.mjs';

function environment() {
    class Socket extends EventTarget {
        sent = [];
        send(value) {
            this.sent.push(value);
        }
        close() {
            this.closed = true;
        }
    }
    class SocketStream {
        sent = [];
        constructor() {
            this.connection = {
                readable: new ReadableStream({ start: controller => (this.controller = controller) }),
                writable: new WritableStream({ write: value => this.sent.push(value) }),
            };
        }
        get opened() {
            return Promise.resolve(this.connection);
        }
        close() {
            this.controller.close();
        }
    }
    const scope = {
        WebSocket: Socket,
        WebSocketStream: SocketStream,
        Blob: class extends Blob {},
        ReadableStream,
        WritableStream,
        TransformStream,
        MessageEvent,
    };
    scope.window = scope;
    runInNewContext(`(${installFaults.toString()})()`, scope);
    return scope;
}

async function until(check) {
    for (let attempt = 0; attempt < 100; attempt++) {
        if (check()) return;
        await delay(1);
    }
    assert.fail('Fault boundary was not reached');
}

test('file-read gate reaches a pending boundary and resumes the original bytes', async () => {
    const browser = environment();
    const fault = browser.integrationFault;
    fault.holdReads = true;
    let complete = false;
    const pending = new browser.Blob(['GPX']).arrayBuffer().then(bytes => {
        complete = true;
        return bytes;
    });
    await until(() => fault.reads.length === 1);
    assert.equal(complete, false);
    fault.release('reads');
    assert.equal(Buffer.from(await pending).toString(), 'GPX');
});

test('classic WebSocket responses replay once in order and disconnect discards held replies', () => {
    const browser = environment();
    const fault = browser.integrationFault;
    const socket = new browser.WebSocket();
    const delivered = [];
    socket.addEventListener('message', event => delivered.push(event.data));
    fault.holdIncoming = true;
    for (const data of ['first', 'second']) socket.dispatchEvent(new MessageEvent('message', { data }));
    assert.deepEqual(delivered, []);
    assert.equal(fault.incoming.length, 2);
    fault.release('incoming');
    assert.deepEqual(delivered, ['first', 'second']);
    fault.holdIncoming = true;
    socket.dispatchEvent(new MessageEvent('message', { data: 'lost' }));
    fault.disconnect();
    assert.equal(socket.closed, true);
    assert.deepEqual(delivered, ['first', 'second']);
    assert.equal(fault.incoming.length, 0);
});

test('WebSocketStream gates real writes and drops delayed responses on disconnect', async () => {
    const browser = environment();
    const fault = browser.integrationFault;
    const socket = new browser.WebSocketStream();
    const connection = await socket.opened;
    const reader = connection.readable.getReader();
    const writer = connection.writable.getWriter();
    fault.holdOutgoing = true;
    const write = writer.write('request');
    await until(() => fault.outgoing.length === 1);
    assert.deepEqual(socket.sent, []);
    fault.release('outgoing');
    await write;
    assert.deepEqual(socket.sent, ['request']);
    fault.holdIncoming = true;
    const read = reader.read();
    socket.controller.enqueue('committed reply');
    await until(() => fault.incoming.length === 1);
    fault.disconnect();
    assert.equal((await read).done, true);
    await reader.cancel();
    await writer.close();
    const replacement = new browser.WebSocketStream();
    const replacementReader = (await replacement.opened).readable.getReader();
    replacement.controller.enqueue('new connection');
    assert.equal((await replacementReader.read()).value, 'new connection');
    replacement.close();
    await replacementReader.cancel();
});
