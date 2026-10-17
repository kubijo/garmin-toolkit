import { rendererSnapshot } from './map-composition.js';
import { routeWindowCommand } from './window-control.js';

export function launchAutomation(name, browser = window) {
    browser.garminAutomation.start(name);
}

// Native HTTP and the browser hooks share the same orchestration and driver.
export function executeAutomation(commandJson, browser = window) {
    try {
        const request = JSON.parse(commandJson);
        const { operation, argument } = request;
        if (!browser.garminAutomation) throw Error('automation is disabled');
        return JSON.stringify({
            value: routeWindowCommand(request, browser, api => {
                if (
                    !['list', 'start', 'status', 'result', 'cancel', 'targets', 'action', 'sequence'].includes(
                        operation,
                    )
                ) {
                    throw Error('unsupported automation operation');
                }
                const value = api[operation](argument);
                const acknowledgement = ['start', 'action', 'sequence', 'cancel'].includes(operation);
                return acknowledgement ? null : (value ?? null);
            }),
        });
    } catch (error) {
        return JSON.stringify({ error: String(error?.message ?? error) });
    }
}

// eframe can have a pending animation callback when an opted-in run starts.
//
// Keep that callback reachable even if Chrome stops delivering
// animation frames while still reporting document.hidden === false.
//
// Only demo automation installs this adapter;
// ordinary runs use the browser's original frame scheduling.
function installFrameFallback(browser) {
    const request = browser.requestAnimationFrame.bind(browser);
    const cancel = browser.cancelAnimationFrame.bind(browser);
    const pending = new Map();
    let enabled = false;

    const deliver = (id, timestamp) => {
        const frame = pending.get(id);
        if (!frame) return;
        pending.delete(id);
        cancel(id);
        browser.clearTimeout(frame.timer);
        frame.callback.call(browser, timestamp);
    };
    const arm = (id, frame) => {
        frame.timer = browser.setTimeout(() => deliver(id, browser.performance.now()), 16);
    };
    browser.requestAnimationFrame = callback => {
        const id = request(timestamp => deliver(id, timestamp));
        const frame = { callback, timer: undefined };
        pending.set(id, frame);
        if (enabled) arm(id, frame);
        return id;
    };
    browser.cancelAnimationFrame = id => {
        const frame = pending.get(id);
        if (frame) {
            pending.delete(id);
            browser.clearTimeout(frame.timer);
        }
        cancel(id);
    };
    return active => {
        if (enabled === active) return;
        enabled = active;
        for (const [id, frame] of pending) {
            if (active) arm(id, frame);
            else {
                browser.clearTimeout(frame.timer);
                frame.timer = undefined;
            }
        }
    };
}

export function installAutomation(command, browser = window) {
    if (browser.garminAutomation) throw Error('automation already installed');
    const backgroundFrames = installFrameFallback(browser);
    let timer;
    let phase;
    let environment;
    let startedAt;
    let pausedAt;
    let pausedMs = 0;
    let runInBackground = false;
    const invoke = (operation, argument = '') => {
        const response = JSON.parse(command(operation, argument));
        if (response.error) throw Error(response.error);
        return response.value;
    };
    const metadata = () => ({
        renderer: rendererSnapshot(),
        viewport: [browser.innerWidth, browser.innerHeight],
        dpr: browser.devicePixelRatio,
    });
    const status = () => {
        const report = invoke('status');
        return report ? { ...report, environment, currentEnvironment: metadata() } : null;
    };
    const observe = () => {
        let report = status();
        if (!report) return;
        if (report.state === 'running' && browser.performance.now() - startedAt - pausedMs > 125000) {
            invoke('cancel', 'scenario watchdog expired');
            report = status();
        }
        const needsFrames = report.needs_background_frames === true;
        backgroundFrames(needsFrames);
        const next = `${report.state}:${report.phase}`;
        if (phase !== next) {
            phase = next;
            browser.performance.mark('garmin.automation.phase', { detail: report });
        }
        if (!['running', 'paused'].includes(report.state) && !needsFrames) {
            browser.clearInterval(timer);
            timer = undefined;
        }
    };
    const cancel = (reason = 'cancelled through browser API') => {
        invoke('cancel', reason);
        observe();
    };
    const updateVisibility = () => {
        const now = browser.performance.now();
        if (browser.document.hidden && !runInBackground) {
            if (pausedAt === undefined) pausedAt = now;
            invoke('pause', JSON.stringify(now / 1000));
        } else {
            if (pausedAt !== undefined) pausedMs += now - pausedAt;
            pausedAt = undefined;
            invoke('resume', JSON.stringify(now / 1000));
        }
        observe();
    };
    browser.document.addEventListener('visibilitychange', updateVisibility);
    const begin = (operation, argument, name) => {
        invoke(operation, typeof argument === 'string' ? argument : JSON.stringify(argument));
        runInBackground = argument?.run_in_background === true;
        startedAt = browser.performance.now();
        pausedMs = 0;
        pausedAt = undefined;
        environment = metadata();
        phase = undefined;
        browser.performance.mark('garmin.automation.start', { detail: { name, ...environment } });
        if (browser.document.hidden && !runInBackground) {
            pausedAt = startedAt;
            invoke('pause', JSON.stringify(startedAt / 1000));
        }
        browser.clearInterval(timer);
        timer = browser.setInterval(observe, 100);
        observe();
        return status();
    };
    const withOptions = (argument, options) =>
        options === undefined ? argument : { run_in_background: false, ...options, argument };
    browser.garminAutomation = Object.freeze({
        list: () => invoke('list'),
        targets: () => invoke('targets'),
        action: (action, options) => begin('action', withOptions(action, options), 'individual-action'),
        sequence: (actions, options) => begin('sequence', withOptions(actions, options), 'custom-sequence'),
        start(name, options) {
            return begin('start', withOptions(name, options), name?.argument ?? name);
        },
        status,
        result() {
            const report = status();
            return report && !['running', 'paused'].includes(report.state) ? report : null;
        },
        cancel: reason => cancel(typeof reason === 'string' ? reason : undefined),
    });
}
