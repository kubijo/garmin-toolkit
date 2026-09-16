const PROTOCOL_VERSION = 1;

let applicationModule;
let initialization;

function postFatal(reason) {
    self.postMessage([PROTOCOL_VERSION, 'fatal', reason instanceof Error ? reason.message : String(reason)]);
}

async function initialize(moduleUrl) {
    if (initialization !== undefined) {
        throw new Error('map worker was initialized more than once');
    }
    initialization = import(moduleUrl).then(async module => {
        await module.default();
        applicationModule = module;
    });
    await initialization;
}

async function prepareTile(module, payload) {
    const [zoom, x, y, darkMode] = payload;
    const tileUrl = new URL(`map/tiles/${zoom}/${x}/${y}.pbf`, self.location.href);
    const response = await fetch(tileUrl);
    if (!response.ok) {
        throw new Error(`map tile request returned HTTP ${response.status}`);
    }
    const encoded = await response.arrayBuffer();
    if (encoded.byteLength > 2 * 1024 * 1024) {
        throw new Error('map tile exceeded the 2 MiB browser limit');
    }
    return module.prepare_map_tile(zoom, darkMode, new Uint8Array(encoded));
}

async function prepareTask(kind, payload) {
    if (applicationModule === undefined) {
        throw new Error('map worker received work before initialization');
    }
    if (kind === 'tile') {
        return prepareTile(applicationModule, payload);
    }
    if (kind === 'labels') {
        return applicationModule.prepare_map_labels(new Uint8Array(payload[0]));
    }
    if (kind === 'route') {
        return applicationModule.prepare_map_route(new Uint8Array(payload[0]));
    }
    throw new Error(`unknown map worker task: ${kind}`);
}

self.onmessage = async ({ data }) => {
    if (!Array.isArray(data) || data[0] !== PROTOCOL_VERSION) {
        postFatal('unsupported map worker protocol version');
        return;
    }
    const [, messageKind, ...payload] = data;
    if (messageKind === 'init') {
        try {
            await initialize(payload[0]);
            self.postMessage([PROTOCOL_VERSION, 'ready']);
        } catch (error) {
            postFatal(error);
        }
        return;
    }
    if (messageKind !== 'task') {
        postFatal(`unknown map worker message: ${messageKind}`);
        return;
    }

    const [id, kind, ...taskPayload] = payload;
    try {
        const prepared = await prepareTask(kind, taskPayload);
        const transferable = prepared.buffer;
        self.postMessage([PROTOCOL_VERSION, 'result', id, kind, transferable], [transferable]);
    } catch (error) {
        self.postMessage([
            PROTOCOL_VERSION,
            'task-error',
            id,
            kind,
            error instanceof Error ? error.message : String(error),
        ]);
    }
};
