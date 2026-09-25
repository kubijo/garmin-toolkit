// All markup and record formatting comes from Rust/Askama.
// This only transports updates and bounds the number of retained DOM nodes.

const connection = document.getElementById('connection');
const history = document.getElementById('history');
const url = new URL(location.href);
url.searchParams.set('format', 'html');
let cursor = document.body.dataset.cursor;
let events;

function connect() {
    if (events) return;
    url.searchParams.set('after', cursor);
    const stream = new EventSource(url);
    events = stream;
    connection.textContent = 'Connecting…';
    stream.addEventListener('view', event => {
        if (events !== stream) return;
        const update = JSON.parse(event.data);
        if (update.state !== null) document.getElementById('state').innerHTML = update.state;
        history.insertAdjacentHTML('beforeend', update.records);
        while (history.childElementCount > 256 || history.textContent.length > 512 * 1024)
            history.firstElementChild.remove();
        document.getElementById('status').textContent = update.status;
        cursor = event.lastEventId || cursor;
        connection.textContent = 'Connected';
    });
    stream.onerror = () => {
        if (events === stream) connection.textContent = 'Disconnected. Reconnecting…';
    };
}

addEventListener('pagehide', () => {
    events?.close();
    events = undefined;
    connection.textContent = 'Disconnected.';
});
addEventListener('pageshow', connect);
connect();
