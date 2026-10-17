// Disposable nested-prefix proxy for browser acceptance against a local HASS demo.
import http from 'node:http';
import net from 'node:net';

const [portArg = '8100', prefix = '/api/hassio_ingress/test/nested/', upstreamPortArg = '8099'] = process.argv.slice(2);
const port = Number(portArg);
const upstreamPort = Number(upstreamPortArg);
if (!Number.isInteger(port) || port < 1024 || port > 65535) throw Error('invalid proxy port');
if (!Number.isInteger(upstreamPort) || upstreamPort < 1024 || upstreamPort > 65535)
    throw Error('invalid upstream port');
if (!prefix.startsWith('/') || !prefix.endsWith('/') || prefix.includes('..')) throw Error('invalid prefix');

function backendPath(request) {
    const url = new URL(request.url, `http://${request.headers.host}`);
    if (!url.pathname.startsWith(prefix)) return null;
    const path = `/${url.pathname.slice(prefix.length)}${url.search}`;
    if (/^\/api\/(control|capabilities|browser-response)(\/|$)/.test(path)) return null;
    return path;
}

const server = http.createServer((request, response) => {
    const path = backendPath(request);
    if (!path) {
        response.writeHead(404).end();
        return;
    }
    const upstream = http.request(
        { host: '127.0.0.1', port: upstreamPort, method: request.method, path, headers: request.headers },
        result => {
            response.writeHead(result.statusCode, result.headers);
            result.pipe(response);
        },
    );
    upstream.on('error', error => response.destroy(error));
    request.pipe(upstream);
});

server.on('upgrade', (request, socket, head) => {
    const path = backendPath(request);
    if (!path) {
        socket.end('HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n');
        return;
    }
    const upstream = net.connect(upstreamPort, '127.0.0.1', () => {
        upstream.write(`${request.method} ${path} HTTP/1.1\r\n`);
        for (const [name, value] of Object.entries(request.headers)) upstream.write(`${name}: ${value}\r\n`);
        upstream.write('\r\n');
        if (head.length) upstream.write(head);
        socket.pipe(upstream).pipe(socket);
    });
    upstream.on('error', () => socket.destroy());
    socket.on('error', () => upstream.destroy());
});

server.listen(port, '127.0.0.1', () => {
    console.log(`Nested ingress proxy: http://127.0.0.1:${port}${prefix}`);
});
