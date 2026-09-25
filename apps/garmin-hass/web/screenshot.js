import { snapshotComposition } from './map-composition.js';

const MAX_PIXELS = 8 * 1024 * 1024;
const MAX_BYTES = 8 * 1024 * 1024;

function geometry(canvas) {
    const bounds = canvas.getBoundingClientRect();
    return JSON.stringify([
        canvas.width,
        canvas.height,
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        devicePixelRatio,
    ]);
}

export function beginScreenshot() {
    const canvas = document.getElementById('garmin-toolkit');
    if (!canvas || document.hidden) throw Error('application canvas is absent or hidden');
    if (!canvas.width || !canvas.height || canvas.width * canvas.height > MAX_PIXELS)
        throw Error('screenshot exceeds pixel limit');
    return { canvas, geometry: geometry(canvas), map: snapshotComposition() };
}

export function cancelScreenshot(ticket) {
    ticket.map.cancel();
}

export async function finishScreenshot(ticket, rgba, width, height) {
    if (
        !Number.isInteger(width) ||
        !Number.isInteger(height) ||
        width <= 0 ||
        height <= 0 ||
        width * height > MAX_PIXELS ||
        rgba.length !== width * height * 4
    )
        throw Error('invalid screenshot pixels');
    // Copy the WASM memory view before any await permits Rust allocations.
    const pixels = new Uint8ClampedArray(rgba);
    const validate = () => {
        if (
            !ticket.canvas.isConnected ||
            document.hidden ||
            geometry(ticket.canvas) !== ticket.geometry ||
            ticket.canvas.width !== width ||
            ticket.canvas.height !== height
        )
            throw Error('viewport changed during screenshot capture');
        ticket.map.validate();
    };
    validate();
    const mapBlob = await ticket.map.blob;
    validate();
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext('2d');
    if (!context) throw Error('screenshot compositor is unavailable');
    if (mapBlob) {
        const bitmap = await createImageBitmap(mapBlob);
        try {
            validate();
            const { projection, clip, screen } = ticket.map.placement;
            if (
                screen[0] !== width ||
                screen[1] !== height ||
                bitmap.width !== Math.ceil(projection[2]) ||
                bitmap.height !== Math.ceil(projection[3])
            )
                throw Error('map screenshot dimensions do not match the UI frame');
            context.save();
            context.beginPath();
            context.rect(...clip);
            context.clip();
            context.drawImage(bitmap, ...projection);
            context.restore();
        } finally {
            bitmap.close();
        }
    }
    const overlay = new OffscreenCanvas(width, height);
    const overlayContext = overlay.getContext('2d');
    if (!overlayContext) throw Error('screenshot overlay is unavailable');
    overlayContext.putImageData(new ImageData(pixels, width, height), 0, 0);
    context.drawImage(overlay, 0, 0);
    const png = await canvas.convertToBlob({ type: 'image/png' });
    validate();
    if (png.size > MAX_BYTES) throw Error('screenshot exceeds PNG byte limit');
    const bytes = new Uint8Array(await png.arrayBuffer());
    validate();
    return bytes;
}
