import { rendererSnapshot } from './map-experiment.js';

export function launchAutomation(name, browser = window) {
    browser.garminAutomation.start(name);
}

export function installAutomation(command, browser = window) {
    if (browser.garminAutomation) throw Error('automation already installed');
    let timer;
    let phase;
    let environment;
    let startedAt;
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
        if (report.state === 'running' && browser.performance.now() - startedAt > 125000) {
            invoke('cancel', 'scenario watchdog expired');
            report = status();
        }
        const next = `${report.state}:${report.phase}`;
        if (phase !== next) {
            phase = next;
            browser.performance.mark('garmin.automation.phase', { detail: report });
        }
        if (report.state !== 'running') {
            browser.clearInterval(timer);
            timer = undefined;
        }
    };
    const cancel = (reason = 'cancelled through browser API') => {
        invoke('cancel', reason);
        observe();
    };
    browser.addEventListener('blur', () => cancel('window lost focus'));
    browser.document.addEventListener('visibilitychange', () => {
        if (browser.document.hidden) cancel('document hidden');
    });
    browser.garminAutomation = Object.freeze({
        list: () => invoke('list'),
        start(name) {
            if (typeof name !== 'string') throw Error('scenario name must be a string');
            if (browser.document.hidden || !browser.document.hasFocus())
                throw Error('page must be visible and focused');
            invoke('start', name);
            startedAt = browser.performance.now();
            environment = metadata();
            phase = undefined;
            browser.performance.mark('garmin.automation.start', { detail: { name, ...environment } });
            timer = browser.setInterval(observe, 100);
            observe();
            return status();
        },
        status,
        result() {
            const report = status();
            return report?.state !== 'running' ? report : null;
        },
        cancel: () => cancel(),
    });
}
