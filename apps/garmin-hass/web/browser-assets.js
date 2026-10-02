// Build-generated HTML is the sole authority for this page's exact asset graph.
export function assetUrl(name) {
    const value = document.querySelector(`meta[name="garmin-${name}"]`)?.content;
    if (!value) throw Error(`missing fingerprinted browser asset: ${name}`);
    return new URL(value, document.baseURI).href;
}
export function applicationAssets() {
    return {
        moduleUrl: assetUrl('module'),
        wasmUrl: assetUrl('wasm'),
        preparationUrl: assetUrl('map-worker'),
        renderUrl: assetUrl('map-render-worker'),
    };
}
