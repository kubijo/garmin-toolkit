import init from './garmin_hass_web.js';

const message = document.querySelector('#loading-message');
const progress = document.querySelector('#loading-progress');
const detail = document.querySelector('#loading-detail');
const trace = document.querySelector('#loading-trace');

function size(bytes) {
    return new Intl.NumberFormat(undefined, {
        style: 'unit',
        unit: 'megabyte',
        maximumFractionDigits: 1,
    }).format(bytes / 1_000_000);
}

function rate(bytes, started) {
    const seconds = Math.max((performance.now() - started) / 1000, 0.001);
    return `${size(bytes / seconds)}/s`;
}

async function download(url) {
    const response = await fetch(url, { cache: 'no-cache' });
    if (!response.ok) {
        throw new Error(`WASM download failed: HTTP ${response.status}`);
    }

    const total = Number(response.headers.get('Content-Length')) || undefined;
    const started = performance.now();
    if (!response.body) {
        const bytes = new Uint8Array(await response.arrayBuffer());
        detail.textContent = `${size(bytes.length)} downloaded`;
        return bytes;
    }

    const reader = response.body.getReader();
    const chunks = [];
    let received = 0;
    if (total) {
        progress.max = total;
        progress.value = 0;
    }
    for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        received += value.length;
        if (total) progress.value = received;
        const amount = total ? `${size(received)} of ${size(total)}` : size(received);
        detail.textContent = `${amount} · ${rate(received, started)}`;
    }

    const bytes = new Uint8Array(received);
    let offset = 0;
    for (const chunk of chunks) {
        bytes.set(chunk, offset);
        offset += chunk.length;
    }
    return bytes;
}

async function start() {
    trace.textContent = 'Downloading the browser application.';
    const bytes = await download(new URL('./garmin_hass_web_bg.wasm', import.meta.url));
    message.textContent = 'Starting Garmin Toolkit…';
    detail.textContent = 'Download complete';
    trace.textContent = 'Initializing the application. The first frame will replace this screen.';
    await init(bytes);
}

start().catch(error => {
    document.querySelector('#loading').setAttribute('aria-busy', 'false');
    message.textContent = 'Garmin Toolkit could not start.';
    detail.textContent = error instanceof Error ? error.message : String(error);
    trace.textContent = 'Reload the page. If the problem persists, inspect the host log.';
    console.error(error);
});
