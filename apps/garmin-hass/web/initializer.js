function reason(error) {
    if (error instanceof Error) {
        if (error instanceof WebAssembly.RuntimeError && error.message === 'unreachable') {
            return 'The WebAssembly application terminated unexpectedly.';
        }
        return error.name === 'Error' ? error.message : `${error.name}: ${error.message}`;
    }
    return typeof error === 'string' ? error : String(error);
}

export default function () {
    const message = document.querySelector('#loading-message');
    const progress = document.querySelector('#loading-progress');
    const detail = document.querySelector('#loading-detail');
    const trace = document.querySelector('#loading-trace');
    const loading = document.querySelector('#loading');
    const retry = document.querySelector('#loading-retry');
    let state = 'loading';
    let pendingProgress;
    let progressFrame;
    const started = performance.now();

    function cancelProgress() {
        if (progressFrame !== undefined) {
            cancelAnimationFrame(progressFrame);
            progressFrame = undefined;
        }
        pendingProgress = undefined;
    }

    function showFailure(error) {
        if (!loading?.isConnected) {
            return false;
        }
        if (state === 'failure') {
            return true;
        }
        state = 'failure';
        console.error(error);
        cancelProgress();
        loading.hidden = false;
        loading.dataset.state = 'failure';
        loading.setAttribute('aria-busy', 'false');
        loading.setAttribute('role', 'alert');
        message.textContent = 'Garmin Toolkit could not start.';
        detail.textContent = reason(error);
        trace.textContent = 'Reload the page. If the problem persists, inspect the browser console and host log.';
        retry.hidden = false;
        return true;
    }

    function size(bytes) {
        return new Intl.NumberFormat(undefined, {
            style: 'unit',
            unit: 'megabyte',
            maximumFractionDigits: 1,
        }).format(bytes / 1_000_000);
    }

    retry.addEventListener('click', () => window.location.reload());
    window.addEventListener('garmin-toolkit-failure', event => showFailure(event.detail));
    window.addEventListener('garmin-toolkit-ready', () => {
        if (state !== 'loading') return;
        state = 'ready';
        cancelProgress();
        loading.hidden = true;
    });
    window.addEventListener('error', event => {
        if (showFailure(event.error ?? event.message)) {
            event.preventDefault();
        }
    });
    window.addEventListener('unhandledrejection', event => {
        if (showFailure(event.reason)) {
            event.preventDefault();
        }
    });

    function renderProgress() {
        progressFrame = undefined;
        if (state !== 'loading' || pendingProgress === undefined) return;
        const { current, total } = pendingProgress;
        const done = total > 0 && current >= total;
        if (total > 0) {
            const ratio = Math.min(current / total, 1);
            progress.dataset.determinate = 'true';
            progress.style.setProperty('--loading-progress', ratio);
            progress.setAttribute('aria-valuemin', '0');
            progress.setAttribute('aria-valuemax', String(total));
            progress.setAttribute('aria-valuenow', String(current));
        }
        if (done) {
            message.textContent = 'Starting Garmin Toolkit…';
            detail.textContent = 'Download complete';
            trace.textContent = 'Initializing the application. The first frame will replace this screen.';
            return;
        }
        const seconds = Math.max((performance.now() - started) / 1000, 0.001);
        const amount = total > 0 ? `${size(current)} of ${size(total)}` : size(current);
        detail.textContent = `${amount} · ${size(current / seconds)}/s`;
    }

    return {
        onStart() {
            if (state !== 'loading') return;
            detail.textContent = 'Starting download…';
            trace.textContent = 'Fetching the browser application from the host.';
        },
        onProgress({ current, total }) {
            if (state !== 'loading') return;
            pendingProgress = { current, total };
            if (progressFrame === undefined) {
                progressFrame = requestAnimationFrame(renderProgress);
            }
        },
        onFailure(error) {
            showFailure(error);
        },
    };
}
