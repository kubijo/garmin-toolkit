// Trunk's post-build gate. esbuild owns JavaScript dependency rewriting and transitive hashes.
// In particular, wasm-bindgen snippet directory names are NOT content hashes.
import { spawnSync } from 'node:child_process';
import { readFile, writeFile, readdir, mkdir, rename, rm } from 'node:fs/promises';
import { dirname, extname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

async function files(root, directory = '') {
    const result = [];
    for (const entry of await readdir(join(root, directory), { withFileTypes: true })) {
        const path = join(directory, entry.name);
        if (entry.isDirectory()) result.push(...(await files(root, path)));
        else if (entry.isFile()) result.push(path);
        else throw Error(`unsupported browser asset: ${path}`);
    }
    return result.sort();
}

export async function fingerprintWeb(directory, esbuild = process.env.ESBUILD ?? 'esbuild') {
    const root = resolve(directory);
    const original = await files(root);
    if (!original.includes('index.html')) throw Error('browser staging directory has no index.html');
    let html = await readFile(join(root, 'index.html'), 'utf8');
    const wasm = original.filter(path => path.endsWith('.wasm'));
    const application = original.filter(path => /^garmin-hass-web-[a-z0-9]+\.js$/.test(path));
    if (wasm.length !== 1 || application.length !== 1) throw Error('expected one Trunk application module and WASM');
    const names = new Map();
    const output = join(root, '.fingerprinted');
    await mkdir(output); // Fail rather than mixing with any previous/stale build output.
    const resources = original.filter(path => !['.html', '.js'].includes(extname(path)));
    const resourceEntry = '__resources.js';
    if (original.includes(resourceEntry)) throw Error('reserved build entrypoint already exists');
    await writeFile(
        join(root, resourceEntry),
        resources
            .map((path, index) => `export { default as asset${index} } from ${JSON.stringify(`./${path}`)};`)
            .join('\n'),
    );
    // Adapt bindgen's implicit WASM URL to a standard file-loader import. esbuild then owns
    // BOTH its output name and the dependent JS hash, including implicit initialization.
    const gluePath = join(root, application[0]);
    let glue = await readFile(gluePath, 'utf8');
    const fallback = /module_or_path\s*=\s*new URL\(['"]garmin[-_]hass[-_]web_bg\.wasm['"],\s*import\.meta\.url\);/g;
    if ([...glue.matchAll(fallback)].length !== 1) throw Error('unrecognized wasm-bindgen implicit WASM URL');
    glue =
        `import __garminWasmUrl from ${JSON.stringify(`./${wasm[0]}`)};\n` +
        glue.replace(fallback, 'module_or_path = new URL(__garminWasmUrl, import.meta.url);');
    await writeFile(gluePath, glue);
    const entries = [...original.filter(path => path.endsWith('.js')), resourceEntry];
    const result = spawnSync(
        esbuild,
        [
            ...entries,
            '--bundle',
            '--splitting',
            '--format=esm',
            '--platform=browser',
            '--target=es2022',
            '--entry-names=[dir]/[name]-[hash]',
            '--chunk-names=[name]-[hash]',
            '--asset-names=[name]-[hash]',
            ...[...new Set(resources.map(extname))].map(extension => `--loader:${extension}=file`),
            '--outdir=.fingerprinted',
            '--metafile=.fingerprinted/metafile.json',
            '--log-level=warning',
            ...(process.env.TRUNK_PROFILE === 'release' ? ['--minify'] : []),
        ],
        { cwd: root, encoding: 'utf8' },
    );
    if (result.error || result.status !== 0)
        throw Error(`browser asset bundling failed: ${result.error ?? result.stderr}`);
    const metadata = JSON.parse(await readFile(join(output, 'metafile.json'), 'utf8'));
    for (const [path, info] of Object.entries(metadata.outputs)) {
        const emitted = relative('.fingerprinted', path);
        if (info.entryPoint) names.set(info.entryPoint, emitted);
        if (!info.entryPoint && extname(emitted) !== '.js') {
            const inputs = Object.keys(info.inputs);
            if (inputs.length !== 1 || !resources.includes(inputs[0])) throw Error(`unknown emitted asset: ${path}`);
            names.set(inputs[0], emitted);
        }
        for (const dependency of info.imports) {
            if (dependency.external || !metadata.outputs[dependency.path])
                throw Error(`unresolved browser import: ${dependency.path}`);
        }
    }
    for (const entry of [...entries, ...resources])
        if (!names.has(entry)) throw Error(`missing fingerprinted asset: ${entry}`);
    // Only the generated HTML is rewritten here; JavaScript syntax and import keys belong to esbuild.
    for (const [before, after] of names) html = html.replaceAll(`./${before}`, `./${after}`);
    const assets = {
        module: application[0],
        wasm: wasm[0],
        'map-worker': 'map-worker.js',
        'map-render-worker': 'map-render-worker.js',
    };
    const tags = Object.entries(assets)
        .map(([name, path]) => {
            const target = names.get(path);
            if (!target) throw Error(`missing browser asset: ${name}`);
            return `<meta name="garmin-${name}" content="./${target}">`;
        })
        .join('');
    html = html.replace('</head>', `${tags}</head>`);
    await writeFile(join(output, 'index.html'), html);
    await rm(join(output, 'metafile.json'));
    // Exact files from this verified staging inventory only; no recursive workspace deletion.
    for (const path of [...original, resourceEntry]) await rm(join(root, path));
    for (const path of await files(output)) {
        await mkdir(dirname(join(root, path)), { recursive: true });
        await rename(join(output, path), join(root, path));
    }
    // Empty intermediate directories are harmless and are not published as assets.
    return Object.fromEntries(names);
}

export function browserAssetPaths(html) {
    return Object.fromEntries(
        [...html.matchAll(/<meta name="garmin-([a-z-]+)" content="\.\/([^"<>]+)">/g)].map(([, name, path]) => [
            name,
            path,
        ]),
    );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    if (!process.env.TRUNK_STAGING_DIR) throw Error('fingerprinting requires Trunk staging context');
    await fingerprintWeb(process.env.TRUNK_STAGING_DIR);
}
