import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { Protocol } from 'devtools-protocol';
import type { ProtocolMapping } from 'devtools-protocol/types/protocol-mapping.js';
import type { BrowserFunction, Clip, Until } from './haos-types.mts';

type Commands = ProtocolMapping.Commands;
type Connection = EventTarget & Pick<WebSocket, 'readyState' | 'send'>;
type Event = {
    [M in keyof ProtocolMapping.Events]: { method: M; params: ProtocolMapping.Events[M][0] };
}[keyof ProtocolMapping.Events];
type RecordedEvent =
    | { method: 'Page.fileChooserOpened'; params: Protocol.Page.FileChooserOpenedEvent }
    | { method: 'Network.webSocketCreated'; params: Protocol.Network.WebSocketCreatedEvent }
    | {
          method: 'Network.webSocketFrameReceived';
          params: Pick<Protocol.Network.WebSocketFrameReceivedEvent, 'requestId'>;
      };
type Response = { id: number; result: unknown; error?: { message: string } };
type Context = Omit<Protocol.Runtime.ExecutionContextDescription, 'auxData'> & {
    auxData?: { isDefault?: boolean; frameId?: string };
};
type Pending = {
    method: string;
    resolve(value: unknown): void;
    reject(error: Error): void;
    timer: ReturnType<typeof setTimeout>;
};

export class DevTools {
    nextId = 0;
    pending = new Map<number, Pending>();
    events: RecordedEvent[] = [];
    contexts = new Map<number, Context>();
    recording = true;
    socket: Connection;
    evidencePath: string;
    until: Until;

    constructor(connection: Connection, evidencePath: string, until: Until) {
        this.socket = connection;
        this.evidencePath = evidencePath;
        this.until = until;
        connection.addEventListener('message', event => {
            assert.ok(event instanceof MessageEvent && typeof event.data === 'string');
            const message = JSON.parse(event.data) as Event | Response;
            if (!('id' in message)) {
                if (message.method === 'Runtime.executionContextCreated')
                    this.contexts.set(message.params.context.id, message.params.context);
                if (message.method === 'Runtime.executionContextDestroyed')
                    this.contexts.delete(message.params.executionContextId);
                if (message.method === 'Runtime.executionContextsCleared') this.contexts.clear();
                if (message.method === 'Page.fileChooserOpened') this.events.push(message);
                else if (this.recording && message.method === 'Network.webSocketCreated')
                    this.events.push({
                        method: message.method,
                        params: {
                            requestId: message.params.requestId,
                            url: message.params.url,
                        },
                    });
                else if (this.recording && message.method === 'Network.webSocketFrameReceived')
                    this.events.push({ method: message.method, params: { requestId: message.params.requestId } });
                return;
            }
            const pending = this.pending.get(message.id);
            if (!pending) return;
            this.pending.delete(message.id);
            clearTimeout(pending.timer);
            if (message.error) pending.reject(Error(`${pending.method}: ${message.error.message}`));
            else pending.resolve(message.result);
        });
        connection.addEventListener('close', () => {
            for (const pending of this.pending.values()) {
                clearTimeout(pending.timer);
                pending.reject(Error('Owned Chrome disconnected'));
            }
            this.pending.clear();
        });
    }

    call<M extends keyof Commands>(
        method: M,
        ...parameters: Commands[M]['paramsType']
    ): Promise<Commands[M]['returnType']> {
        const id = ++this.nextId;
        return new Promise((resolveCall, reject) => {
            const timer = setTimeout(() => {
                this.pending.delete(id);
                reject(Error(`DevTools command timed out: ${method}`));
            }, 15_000);
            this.pending.set(id, {
                resolve: value => resolveCall(value as Commands[M]['returnType']),
                reject,
                timer,
                method,
            });
            try {
                this.socket.send(JSON.stringify({ id, method, params: parameters[0] ?? {} }));
            } catch (error) {
                clearTimeout(timer);
                this.pending.delete(id);
                reject(error);
            }
        });
    }

    evaluate<R>(fn: () => R, argument?: undefined, contextId?: number): Promise<Awaited<R>>;
    evaluate<A, R>(fn: BrowserFunction<A, R>, argument: A, contextId?: number): Promise<Awaited<R>>;
    async evaluate(fn: { toString(): string }, argument?: unknown, contextId?: number): Promise<unknown> {
        const window = await this.call('Runtime.evaluate', {
            expression: 'window',
            ...(contextId === undefined ? {} : { contextId }),
        });
        if (window.exceptionDetails)
            throw Error(
                window.exceptionDetails.exception?.description ??
                    String(window.exceptionDetails.exception?.value ?? window.exceptionDetails.text),
            );
        assert.ok(window.result.objectId, 'the requested browser window is available');
        const reply = await this.call('Runtime.callFunctionOn', {
            objectId: window.result.objectId,
            functionDeclaration: fn.toString(),
            arguments: [{ value: argument }],
            returnByValue: true,
            awaitPromise: true,
            userGesture: true,
        });
        if (reply.exceptionDetails)
            throw Error(
                reply.exceptionDetails.exception?.description ??
                    String(reply.exceptionDetails.exception?.value ?? reply.exceptionDetails.text),
            );
        return reply.result.value as unknown;
    }

    async capture(name: string, clip?: Clip) {
        const result = await this.until(
            () =>
                this.call('Page.captureScreenshot', {
                    format: 'png',
                    ...(clip ? { clip: { ...clip, scale: 1 } } : {}),
                }),
            'active page screenshot',
            30_000,
        );
        await writeFile(resolve(this.evidencePath, name), Buffer.from(result.data, 'base64'));
    }
}
