import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';
import { DevTools } from './haos-devtools.mts';
import type { Until } from './haos-types.mts';

class Connection extends EventTarget {
    readyState = WebSocket.OPEN;
    commands: { id: number; method: string; params: Record<string, unknown> }[] = [];
    failSend = false;
    send(data: unknown) {
        if (this.failSend) throw Error('closed transport');
        assert.equal(typeof data, 'string');
        this.commands.push(JSON.parse(data as string) as (typeof this.commands)[number]);
    }
    message(message: unknown) {
        this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(message) }));
    }
    reply(result: unknown, error?: { message: string }) {
        const command = this.commands.at(-1);
        assert.ok(command);
        this.message({ id: command.id, result, error });
    }
}

const unusedUntil: Until = async () => {
    throw Error('unexpected screenshot request');
};
const connection = new Connection();
const client = new DevTools(connection, '.tmp', unusedUntil);
try {
    const enabled = client.call('Page.enable');
    assert.deepEqual(connection.commands.at(-1)?.params, {});
    connection.reply({});
    await enabled;

    const failure = client.call('Page.navigate', { url: 'https://example.invalid' });
    connection.reply(undefined, { message: 'navigation failed' });
    await assert.rejects(failure, /Page.navigate: navigation failed/);

    connection.message({
        method: 'Runtime.executionContextCreated',
        params: {
            context: {
                id: 7,
                origin: 'http://127.0.0.1',
                name: '',
                uniqueId: 'owned',
                auxData: { isDefault: true, frameId: 'app' },
            },
        },
    });
    assert.equal(client.contexts.get(7)?.auxData?.frameId, 'app');
    connection.message({ method: 'Runtime.executionContextDestroyed', params: { executionContextId: 7 } });
    assert.equal(client.contexts.size, 0);

    connection.message({
        method: 'Network.webSocketFrameReceived',
        params: { requestId: 'owned', response: { payloadData: 'private data' } },
    });
    assert.deepEqual(client.events, [{ method: 'Network.webSocketFrameReceived', params: { requestId: 'owned' } }]);
    client.recording = false;
    connection.message({ method: 'Network.webSocketCreated', params: { requestId: 'ignored', url: 'ws://127.0.0.1' } });
    connection.message({
        method: 'Page.fileChooserOpened',
        params: { frameId: 'app', backendNodeId: 23, mode: 'selectSingle' },
    });
    assert.equal(client.events.length, 2, 'file choosers stay observable after network recording ends');

    const evaluated = client.evaluate(
        async (value: { name: string }) => {
            const node = document.querySelector<HTMLCanvasElement>('#garmin-toolkit');
            return { name: value.name, width: node?.width };
        },
        { name: 'typed callback' },
        7,
    );
    assert.deepEqual(connection.commands.at(-1)?.params, { expression: 'window', contextId: 7 });
    connection.reply({ result: { objectId: 'owned-window' } });
    await Promise.resolve();
    const command = connection.commands.at(-1);
    assert.equal(command?.method, 'Runtime.callFunctionOn');
    assert.equal(command.params.objectId, 'owned-window');
    assert.equal(command.params.awaitPromise, true);
    const callback = runInNewContext(`(${command.params.functionDeclaration})`, {
        document: { querySelector: () => ({ width: 1280 }) },
    }) as (value: { name: string }) => Promise<{ name: string; width: number }>;
    const result = await callback({ name: 'typed callback' });
    connection.reply({ result: { value: result } });
    assert.deepEqual(await evaluated, { name: 'typed callback', width: 1280 });

    const exception = client.evaluate(() => true);
    connection.reply({ exceptionDetails: { text: 'Execution context was destroyed' } });
    await assert.rejects(exception, /Execution context was destroyed/);

    const disconnected = client.call('Runtime.enable');
    connection.dispatchEvent(new Event('close'));
    await assert.rejects(disconnected, /Owned Chrome disconnected/);
    assert.equal(client.pending.size, 0);
    connection.failSend = true;
    await assert.rejects(client.call('Page.enable'), /closed transport/);
    assert.equal(client.pending.size, 0, 'failed sends leave no pending timer');
} finally {
    connection.dispatchEvent(new Event('close'));
}
console.log('Typed HAOS DevTools transport and stripped browser callbacks: passed');
