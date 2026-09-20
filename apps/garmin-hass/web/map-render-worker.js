import { compositionMessage, decodeComposition, installMapDrawRequest } from './worker-codec.js';

// Worker-owned wgpu surface for the composition fixture and production map experiment.
export function installCompositionWorker(scope) {
    let renderer;
    let initialized = false;
    let failed = false;
    let map = false;
    let active = false;
    let dimensions;
    let timer;
    let due = Infinity;

    function schedule(milliseconds) {
        if (!map || !active || failed || !dimensions || !Number.isFinite(milliseconds)) return;
        const next = performance.now() + Math.max(0, milliseconds);
        if (next >= due) return;
        clearTimeout(timer);
        due = next;
        timer = setTimeout(
            () => {
                due = Infinity;
                try {
                    renderer.draw(...dimensions);
                } catch (error) {
                    fail(error);
                }
            },
            Math.max(16, milliseconds),
        );
    }
    installMapDrawRequest(schedule);

    function fail(error) {
        if (failed) return;
        failed = true;
        clearTimeout(timer);
        // A WASM trap can leave the Rust borrow guard held. Calling free() then throws
        // again and hides the original failure. The host terminates this failed worker,
        // releasing its entire WASM instance and GPU resources instead.
        renderer = undefined;
        scope.postMessage(compositionMessage('composition-failed', String(error).slice(0, 1024)));
    }

    scope.onmessage = async ({ data }) => {
        if (failed) return;
        try {
            const message = decodeComposition(data);
            if (['composition-init', 'map-init'].includes(message.type)) {
                if (initialized) throw Error('composition worker initialized twice');
                initialized = true;
                const module = await import(message.moduleUrl);
                await module.default({ module_or_path: message.wasmUrl });
                if (failed) return;
                renderer = await module.CompositionRenderer.create(message.canvas, message.mode);
                if (failed) {
                    renderer.free();
                    renderer = undefined;
                    return;
                }
                map = message.type === 'map-init';
                if (map) renderer.enable_map(message.moduleUrl, message.wasmUrl, message.preparationUrl);
                scope.postMessage(compositionMessage('composition-ready', renderer.backend(), renderer.maximum_size()));
            } else if (message.type === 'map-active' && map && renderer) {
                active = message.active;
                if (active) schedule(0);
                else {
                    clearTimeout(timer);
                    due = Infinity;
                }
            } else if (message.type === 'map-frame' && map && renderer) {
                renderer.update_map(
                    message.view,
                    message.route === null ? new Uint8Array(0) : new Uint8Array(message.route),
                );
                dimensions = [message.width, message.height];
                active = true;
                renderer.draw(...dimensions);
                scope.postMessage(compositionMessage('composition-drawn', message.id, ...dimensions));
            } else if (message.type === 'composition-size' && !map && renderer) {
                renderer.draw(message.width, message.height);
                // Acknowledges submission, NOT compositor presentation.
                scope.postMessage(compositionMessage('composition-drawn', message.id, message.width, message.height));
            } else {
                throw Error('composition work arrived before readiness or has the wrong direction');
            }
        } catch (error) {
            fail(error);
        }
    };
}

if (typeof self !== 'undefined' && typeof document === 'undefined') installCompositionWorker(self);
