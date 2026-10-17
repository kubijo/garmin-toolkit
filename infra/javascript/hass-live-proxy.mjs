// Disposable development bridge: fresh Trunk HTML/assets plus the HASS demo backend.
import http from 'node:http';
import net from 'node:net';

const listenPort = 18101;
const trunkPort = 18100;
const hostPort = 18099;
const replacements = new Map([
    ['GARMIN_TOOLKIT_MAP_UPLOAD_TELEMETRY', 'false'],
    ['GARMIN_TOOLKIT_MAP_RENDER_WORKER', 'true'],
    ['GARMIN_TOOLKIT_UI_AUTOMATION', 'true'],
]);

function isBackend(path) {
    return path === '/remoc' || path.startsWith('/api/');
}

const server = http.createServer((request, response) => {
    const path = new URL(request.url, `http://${request.headers.host}`).pathname;
    const entrypoint = path === '/' || path === '/index.html';
    const port = isBackend(path) ? hostPort : trunkPort;
    const headers = { ...request.headers };
    if (port === trunkPort) headers.host = `127.0.0.1:${port}`;
    if (entrypoint) headers['accept-encoding'] = 'identity';
    const upstream = http.request(
        { host: '127.0.0.1', port, method: request.method, path: request.url, headers },
        result => {
            if (!entrypoint || request.method !== 'GET' || result.statusCode !== 200) {
                response.writeHead(result.statusCode, result.headers);
                result.pipe(response);
                return;
            }
            const chunks = [];
            result.on('data', chunk => chunks.push(chunk));
            result.on('end', () => {
                let html = Buffer.concat(chunks).toString('utf8');
                for (const [placeholder, value] of replacements) {
                    if (!html.includes(placeholder)) {
                        response.writeHead(502).end(`missing ${placeholder} in Trunk entrypoint`);
                        return;
                    }
                    html = html.replaceAll(placeholder, value);
                }
                response.writeHead(200, {
                    'content-type': 'text/html; charset=utf-8',
                    'cache-control': 'no-store',
                    'content-length': Buffer.byteLength(html),
                });
                response.end(html);
            });
        },
    );
    upstream.on('error', error => response.writeHead(502).end(error.message));
    request.pipe(upstream);
});

server.on('upgrade', (request, socket, head) => {
    const path = new URL(request.url, `http://${request.headers.host}`).pathname;
    const port = isBackend(path) ? hostPort : trunkPort;
    const upstream = net.connect(port, '127.0.0.1', () => {
        upstream.write(`${request.method} ${request.url} HTTP/1.1\r\n`);
        for (const [name, value] of Object.entries(request.headers)) {
            if (name === 'host' && port === trunkPort) upstream.write(`host: 127.0.0.1:${port}\r\n`);
            else upstream.write(`${name}: ${value}\r\n`);
        }
        upstream.write('\r\n');
        if (head.length) upstream.write(head);
        socket.pipe(upstream).pipe(socket);
    });
    upstream.on('error', () => socket.destroy());
    socket.on('error', () => upstream.destroy());
});

server.listen(listenPort, '127.0.0.1', () => {
    console.log(`Disposable HASS live bridge: http://127.0.0.1:${listenPort}/`);
});
