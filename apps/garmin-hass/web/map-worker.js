import { recordLog } from './logging.js';
import * as codec from './worker-codec.js';

export function installMapWorker(scope) {
    let applicationModule;
    let initialization;
    let protocolVersion;
    const reason = error => (error instanceof Error ? error.message : String(error));
    const fatal = error => {
        recordLog('Error', 'map-worker', reason(error), 'preparation-worker');
        scope.postMessage(codec.fatal(protocolVersion ?? 0, reason(error)));
    };

    async function prepare(task) {
        if (applicationModule === undefined) throw Error('map worker received work before initialization');
        if (task.kind === 'tile') {
            const url = new URL(`map/tiles/${task.zoom}/${task.x}/${task.y}.pbf`, scope.location.href);
            const response = await fetch(url);
            if (!response.ok) throw Error(`map tile request returned HTTP ${response.status}`);
            return applicationModule.prepare_map_tile(
                task.zoom,
                task.darkMode,
                new Uint8Array(await response.arrayBuffer()),
            );
        }
        const bytes = new Uint8Array(task.buffers[0]);
        return task.kind === 'labels'
            ? applicationModule.prepare_map_labels(bytes)
            : applicationModule.prepare_map_route(bytes);
    }

    scope.onmessage = async ({ data }) => {
        let task;
        try {
            task = codec.decode(data);
        } catch (error) {
            fatal(error);
            return;
        }
        if (task.type === 'init') {
            try {
                if (initialization !== undefined) throw Error('map worker was initialized more than once');
                initialization = import(task.moduleUrl);
                const module = await initialization;
                await module.default({ module_or_path: task.wasmUrl });
                protocolVersion = module.browser_worker_protocol_version();
                if (task.version !== protocolVersion) throw Error('map worker protocol does not match host');
                applicationModule = module;
                scope.postMessage(codec.ready(protocolVersion));
            } catch (error) {
                fatal(error);
            }
            return;
        }
        if (task.version !== protocolVersion || task.type !== 'task') {
            fatal('map worker protocol or message type does not match host');
            return;
        }
        try {
            const prepared = await prepare(task);
            const parts = task.kind === 'tile' ? prepared : [prepared];
            if (
                !Array.isArray(parts) ||
                parts.some(
                    part =>
                        !(part instanceof Uint8Array) ||
                        part.byteOffset !== 0 ||
                        part.byteLength !== part.buffer.byteLength,
                )
            ) {
                throw Error('map preparation returned a malformed transfer packet');
            }
            const buffers = parts.map(part => part.buffer);
            scope.postMessage(codec.result(protocolVersion, task.id, task.kind, buffers), buffers);
        } catch (error) {
            scope.postMessage(codec.taskError(protocolVersion, task.id, task.kind, reason(error)));
        }
    };
}

if (typeof self !== 'undefined' && typeof document === 'undefined') installMapWorker(self);
