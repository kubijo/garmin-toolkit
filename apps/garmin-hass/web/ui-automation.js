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

export function installAutomation(command, browser = window) {
    if (browser.garminAutomation) throw Error('automation already installed');
    let timer;
    let phase;
    let environment;
    let startedAt;
    let pausedAt;
    let pausedMs = 0;
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
        const next = `${report.state}:${report.phase}`;
        if (phase !== next) {
            phase = next;
            browser.performance.mark('garmin.automation.phase', { detail: report });
        }
        if (!['running', 'paused'].includes(report.state)) {
            browser.clearInterval(timer);
            timer = undefined;
        }
    };
    const cancel = (reason = 'cancelled through browser API') => {
        invoke('cancel', reason);
        observe();
    };
    browser.document.addEventListener('visibilitychange', () => {
        const now = browser.performance.now();
        if (browser.document.hidden) {
            if (pausedAt === undefined) pausedAt = now;
            invoke('pause', JSON.stringify(now / 1000));
        } else {
            if (pausedAt !== undefined) pausedMs += now - pausedAt;
            pausedAt = undefined;
            invoke('resume', JSON.stringify(now / 1000));
        }
        observe();
    });
    const begin = (operation, argument, name) => {
        invoke(operation, argument);
        startedAt = browser.performance.now();
        pausedMs = 0;
        pausedAt = undefined;
        environment = metadata();
        phase = undefined;
        browser.performance.mark('garmin.automation.start', { detail: { name, ...environment } });
        if (browser.document.hidden) {
            pausedAt = startedAt;
            invoke('pause', JSON.stringify(startedAt / 1000));
        }
        browser.clearInterval(timer);
        timer = browser.setInterval(observe, 100);
        observe();
        return status();
    };
    browser.garminAutomation = Object.freeze({
        list: () => invoke('list'),
        targets: () => invoke('targets'),
        action: action => begin('action', JSON.stringify(action), 'individual-action'),
        sequence: actions => begin('sequence', JSON.stringify(actions), 'custom-sequence'),
        start(name) {
            if (typeof name !== 'string') throw Error('scenario name must be a string');
            return begin('start', name, name);
        },
        status,
        result() {
            const report = status();
            return report && !['running', 'paused'].includes(report.state) ? report : null;
        },
        cancel: reason => cancel(typeof reason === 'string' ? reason : undefined),
    });
}
