// Installed only by the integration browser, before the application loads.
export function installFaults({ enabled = true, transport = 'auto' } = {}) {
    if (transport === 'standard') delete window.WebSocketStream;
    const connections = { standard: 0, stream: 0 };
    window.integrationSockets = connections;
    for (const [name, key] of [
        ['WebSocket', 'standard'],
        ['WebSocketStream', 'stream'],
    ]) {
        if (window[name]) {
            window[name] = new Proxy(window[name], {
                construct(target, args, newTarget) {
                    connections[key]++;
                    return Reflect.construct(target, args, newTarget);
                },
            });
        }
    }
    if (!enabled) return;
    const fault = {
        holdReads: false,
        reads: [],
        holdIncoming: false,
        incoming: [],
        holdOutgoing: false,
        outgoing: [],
        sockets: [],
    };
    window.integrationFault = fault;
    const read = Blob.prototype.arrayBuffer;
    Blob.prototype.arrayBuffer = async function () {
        if (fault.holdReads) await new Promise(resolve => fault.reads.push(resolve));
        return read.call(this);
    };
    const NativeStream = window.WebSocketStream;
    if (NativeStream) {
        window.WebSocketStream = class extends NativeStream {
            constructor(...args) {
                super(...args);
                const entry = {
                    discard: false,
                    close: () => this.close({ closeCode: 4000, reason: 'Integration fault' }),
                };
                fault.sockets.push(entry);
                const opened = super.opened.then(connection => {
                    const writer = connection.writable.getWriter();
                    return {
                        ...connection,
                        readable: connection.readable.pipeThrough(
                            new TransformStream({
                                async transform(chunk, controller) {
                                    if (fault.holdIncoming) await new Promise(resolve => fault.incoming.push(resolve));
                                    if (!entry.discard) controller.enqueue(chunk);
                                },
                            }),
                        ),
                        writable: new WritableStream({
                            async write(chunk) {
                                if (fault.holdOutgoing) await new Promise(resolve => fault.outgoing.push(resolve));
                                return writer.write(chunk);
                            },
                            close: () => writer.close(),
                            abort: reason => writer.abort(reason),
                        }),
                    };
                });
                Object.defineProperty(this, 'opened', { value: opened });
            }
        };
    }
    const NativeSocket = window.WebSocket;
    window.WebSocket = class extends NativeSocket {
        constructor(...args) {
            super(...args);
            const entry = { discard: false, close: () => this.close(4000, 'Integration fault') };
            fault.sockets.push(entry);
            this.addEventListener(
                'message',
                event => {
                    if (!fault.holdIncoming || entry.replaying) return;
                    event.stopImmediatePropagation();
                    fault.incoming.push(() => {
                        if (!entry.discard) {
                            entry.replaying = true;
                            try {
                                this.dispatchEvent(new MessageEvent('message', { data: event.data }));
                            } finally {
                                entry.replaying = false;
                            }
                        }
                    });
                },
                true,
            );
        }
        send(data) {
            if (fault.holdOutgoing) fault.outgoing.push(() => super.send(data));
            else super.send(data);
        }
    };
    fault.release = direction => {
        const flag = `hold${direction[0].toUpperCase()}${direction.slice(1)}`;
        fault[flag] = false;
        for (const resume of fault[direction].splice(0)) resume();
    };
    fault.disconnect = () => {
        for (const socket of fault.sockets.splice(0)) {
            socket.discard = true;
            socket.close();
        }
        fault.release('incoming');
        fault.release('outgoing');
        fault.release('reads');
    };
}
