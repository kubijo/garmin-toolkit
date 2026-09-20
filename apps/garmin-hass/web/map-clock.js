// The same admission clock runs in Window and DedicatedWorkerGlobalScope.
// Never resolve synchronously when a window is absent: doing so spins admission and timeouts.
export function waitMilliseconds(milliseconds) {
    return new Promise(resolve => globalThis.setTimeout(resolve, milliseconds));
}

export function nextMapFrame() {
    return new Promise(resolve => {
        if (typeof globalThis.requestAnimationFrame === 'function') globalThis.requestAnimationFrame(resolve);
        else globalThis.setTimeout(resolve, 16);
    });
}
