const message = document.querySelector('#loading-message');
const progress = document.querySelector('#loading-progress');
const detail = document.querySelector('#loading-detail');
const trace = document.querySelector('#loading-trace');
let pendingProgress;
let progressFrame;

function size(bytes) {
    return new Intl.NumberFormat(undefined, {
        style: 'unit',
        unit: 'megabyte',
        maximumFractionDigits: 1,
    }).format(bytes / 1_000_000);
}

export default function () {
    const started = performance.now();
    function renderProgress() {
        progressFrame = undefined;
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
            detail.textContent = 'Starting download…';
            trace.textContent = 'Fetching the browser application from the host.';
        },
        onProgress({ current, total }) {
            pendingProgress = { current, total };
            if (progressFrame === undefined) {
                progressFrame = requestAnimationFrame(renderProgress);
            }
        },
        onFailure(error) {
            if (progressFrame !== undefined) {
                cancelAnimationFrame(progressFrame);
                progressFrame = undefined;
            }
            pendingProgress = undefined;
            document.querySelector('#loading').setAttribute('aria-busy', 'false');
            message.textContent = 'Garmin Toolkit could not start.';
            detail.textContent = error instanceof Error ? error.message : String(error);
            trace.textContent = 'Reload the page. If the problem persists, inspect the host log.';
            console.error(error);
        },
    };
}
