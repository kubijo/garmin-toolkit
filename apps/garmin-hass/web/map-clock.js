// Preparation deadlines count active time in both Window and the render worker.
export class MapClock {
    pending = new Set();

    constructor(active = true) {
        this.active = active;
    }

    setActive(active) {
        if (this.active === active) return;
        this.active = active;
        for (const deadline of this.pending) {
            if (active) this.arm(deadline);
            else {
                clearTimeout(deadline.timer);
                deadline.remaining = Math.max(0, deadline.remaining - (performance.now() - deadline.started));
            }
        }
    }

    wait(milliseconds) {
        return new Promise(resolve => this.schedule(resolve, milliseconds));
    }

    schedule(resolve, milliseconds) {
        const deadline = { remaining: milliseconds, resolve };
        this.pending.add(deadline);
        if (this.active) this.arm(deadline);
        return () => {
            clearTimeout(deadline.timer);
            this.pending.delete(deadline);
        };
    }

    arm(deadline) {
        deadline.started = performance.now();
        deadline.timer = setTimeout(() => {
            this.pending.delete(deadline);
            deadline.resolve();
        }, deadline.remaining);
    }
}

// wasm-bindgen and the worker entry point can import different URLs for this module.
const clockKey = Symbol.for('garmin.map.preparation-clock');
const shared = (globalThis[clockKey] ??= { clock: new MapClock(), visibilityInstalled: false });

export function setMapClockActive(active) {
    shared.clock.setActive(active);
}

export function waitMilliseconds(milliseconds) {
    if (!shared.visibilityInstalled && typeof document !== 'undefined') {
        shared.visibilityInstalled = true;
        const changed = () => setMapClockActive(!document.hidden);
        document.addEventListener('visibilitychange', changed);
        changed();
    }
    return shared.clock.wait(milliseconds);
}

export function nextMapFrame() {
    return new Promise(resolve => {
        if (typeof globalThis.requestAnimationFrame === 'function') globalThis.requestAnimationFrame(resolve);
        else globalThis.setTimeout(resolve, 16);
    });
}
