"""Exercise an already-running HASS demo through its opt-in HTTP control API."""

import ipaddress
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from email.message import Message
from pathlib import Path
from typing import IO, Annotated, Any, Literal

import tyro

from json_data import is_object

MAX_CAPTURE_BYTES = 8 * 1024 * 1024


@dataclass(frozen=True, kw_only=True)
class Command:
    """Send one operation to the app control API."""

    operation: tyro.conf.Positional[str]
    argument: Annotated[str | None, tyro.conf.arg(help='JSON argument, including quotes for strings')] = None
    request_id: Annotated[int | None, tyro.conf.arg(help='Optional explicit ID for replay checks')] = None
    window: Annotated[str | None, tyro.conf.arg(help='Child handle from windows; defaults to root')] = None
    run_in_background: Annotated[bool, tyro.conf.arg(help='Run this automation even in a hidden tab')] = False


@dataclass(frozen=True, kw_only=True)
class Check:
    """Run the HTTP bridge checks."""

    output: Path = Path('.tmp/hass-control-runtime')


@dataclass(frozen=True, kw_only=True)
class Screenshot:
    """Save a screenshot from the app control API."""

    output: Path = Path('.tmp/hass-control-runtime/screenshot.png')
    window: Annotated[str | None, tyro.conf.arg(help='Child handle from windows; defaults to root')] = None


@dataclass(frozen=True, kw_only=True)
class BrowserResponse:
    """Control a browser asset response fault."""

    kind: tyro.conf.Positional[Literal['fail', 'delay', 'unknown_size', 'clear']]
    path: Annotated[str | None, tyro.conf.arg(help='Exact browser asset path, e.g. /index.html')] = None
    milliseconds: Annotated[int | None, tyro.conf.arg(help='Delay in milliseconds, from 1 to 30000')] = None


@dataclass(frozen=True, kw_only=True)
class Options:
    """Exercise an already-running HASS demo through its opt-in HTTP control API."""

    mode: Annotated[
        (
            Annotated[Command, tyro.conf.subcommand('command', prefix_name=False)]
            | Annotated[Check, tyro.conf.subcommand('check', prefix_name=False)]
            | Annotated[Screenshot, tyro.conf.subcommand('screenshot', prefix_name=False)]
            | Annotated[BrowserResponse, tyro.conf.subcommand('browser-response', prefix_name=False)]
        ),
        tyro.conf.arg(name=''),
    ]
    url: str = 'http://127.0.0.1:8099/'


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Keep control requests on the explicitly selected localhost endpoint."""

    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: IO[bytes],
        code: int,
        msg: str,
        headers: Message,
        newurl: str,
    ) -> None:
        return None


class Client:
    def __init__(self, url: str) -> None:
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.query or parsed.fragment:
            raise ValueError('URL must be an HTTP(S) application base URL without query or fragment')
        if parsed.hostname != 'localhost' and not ipaddress.ip_address(parsed.hostname).is_loopback:
            raise ValueError('Control URL must use localhost or a loopback IP address')
        self.url = url.rstrip('/') + '/'
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        self.request_id: int | None = 0

    def request(
        self,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        headers: dict[str, str] | None = None,
    ) -> tuple[int, Any]:
        outgoing = dict(headers or {})
        data = None
        if body is not None:
            outgoing['Content-Type'] = 'application/json'
            data = json.dumps(body).encode()
        request = urllib.request.Request(self.url + path, data=data, headers=outgoing)
        try:
            response = self.opener.open(request, timeout=8)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw_bytes = response.read(MAX_CAPTURE_BYTES + 1)
            if len(raw_bytes) > MAX_CAPTURE_BYTES:
                raise ValueError('Control response exceeds size limit')
            if response.code == 200 and response.headers.get_content_type() == 'image/png':
                metadata = json.loads(response.headers['x-garmin-capture'])
                capture_id = response.headers.get('x-garmin-request-id')
                received_id = int(capture_id) if capture_id is not None else None
                if isinstance(body, dict) and body.get('request_id') is not None and received_id != body['request_id']:
                    raise ValueError('Screenshot came from a different request')
                self.request_id = received_id
                validate_png(raw_bytes, metadata)
                return response.code, {'png': raw_bytes, 'metadata': metadata}
            raw = raw_bytes.decode()
            try:
                value = json.loads(raw)
                if is_object(value) and 'request_id' in value:
                    self.request_id = value['request_id']
            except ValueError:
                value = raw
            return response.code, value

    def command(
        self,
        operation: str,
        argument: object = None,
        *,
        request_id: int | None = None,
        window: str | None = None,
    ) -> tuple[int, Any]:
        return self.request(
            'api/control',
            {
                'request_id': request_id,
                'operation': operation,
                'argument': argument,
                **({'window': window} if window is not None else {}),
            },
        )


def validate_png(png: bytes, metadata: dict[str, Any]) -> None:
    if len(png) < 24 or png[:8] != b'\x89PNG\r\n\x1a\n' or png[12:16] != b'IHDR':
        raise ValueError('Screenshot is not a PNG')
    width, height = int.from_bytes(png[16:20]), int.from_bytes(png[20:24])
    if width != metadata['width'] or height != metadata['height'] or not 0 < width * height <= 8 * 1024 * 1024:
        raise ValueError('Screenshot dimensions do not match its metadata')


def screenshot(client: Client, output: Path, window: str | None = None) -> None:
    if output.suffix.lower() != '.png':
        raise ValueError('Screenshot output must use the .png extension')
    _, reply = expect(client.command('screenshot', window=window), 200)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(reply['png'])
    output.with_suffix('.json').write_text(json.dumps(reply['metadata'], indent=2) + '\n')
    print(f'Saved screenshot: {output}', flush=True)


def expect(reply: tuple[int, Any], status: int) -> tuple[int, Any]:
    if reply[0] != status:
        raise ValueError(f'Expected HTTP {status}, got {reply[0]}: {reply[1]}')
    return reply


def finish(
    client: Client,
    output: Path,
    name: str,
    expected: str = 'passed',
    timeout: float = 40,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        _, reply = expect(client.command('result'), 200)
        report = reply['value']
        if report is not None:
            (output / f'{name}.json').write_text(json.dumps(report, indent=2) + '\n')
            print(f'{name}: {report["state"]} {report["completed"]}/{report["total"]}', flush=True)
            if report['state'] != expected:
                raise ValueError(f'{name}: {report.get("failure")}')
            return report
        time.sleep(0.25)
    raise TimeoutError(f'{name}: no terminal report within {timeout} seconds')


def check(client: Client, output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    _, reply = expect(client.command('status'), 200)
    if reply['value'] and reply['value']['state'] in ('running', 'paused'):
        raise ValueError('The app already has an active workload')
    expect(client.request('api/capabilities', headers={'Origin': client.url}), 403)
    expect(client.request('api/capabilities'), 200)
    expect(client.request('api/control/sessions'), 404)
    expect(client.request('api/capabilities', headers={'X-Forwarded-For': '127.0.0.1'}), 404)
    expect(client.command('unsupported'), 400)
    expect(client.command('start', 'x' * 17_000), 413)
    print('Origin and forwarded request rejection, invalid operation, and size limit: passed', flush=True)

    # Only cancel runs started by this check; never interfere with an existing run.
    owned_run = False
    try:
        for name in ('responsive-layout', 'stationary-arrival', 'warm-interaction', 'activity-smoke'):
            expect(client.command('start', name), 200)
            owned_run = True
            expect(
                client.request(
                    'api/control',
                    {
                        'request_id': client.request_id,
                        'operation': 'start',
                        'argument': name,
                    },
                ),
                409,
            )
            finish(client, output, name)
            owned_run = False

        expect(client.command('action', {'kind': 'assert_available', 'target': 'map.fit'}), 200)
        owned_run = True
        finish(client, output, 'individual-action')
        owned_run = False
        expect(
            client.command(
                'sequence',
                [
                    {'kind': 'assert_available', 'target': 'playback.toggle'},
                    {'kind': 'click', 'target': 'map.fit'},
                    {'kind': 'assert_available', 'target': 'profile.toggle'},
                ],
            ),
            200,
        )
        owned_run = True
        finish(client, output, 'custom-sequence')
        owned_run = False

        expect(client.command('start', 'responsive-layout'), 200)
        owned_run = True
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            _, reply = expect(client.command('status'), 200)
            if reply['value']['viewport'] == [1100, 720] and reply['value']['state'] == 'running':
                break
            time.sleep(0.1)
        else:
            raise TimeoutError('Did not observe the running scenario at its requested size')
        expect(client.command('start', 'activity-smoke'), 400)
        expect(client.command('cancel', 'Runtime cancellation check'), 200)
        finish(client, output, 'cancelled-responsive', expected='cancelled')
        owned_run = False
    finally:
        if owned_run:
            expect(client.command('cancel', 'Runtime check interrupted'), 200)
    print(f'HTTP bridge checks passed. Reports: {output}', flush=True)


def main() -> int:
    args = tyro.cli(Options)
    try:
        client = Client(args.url)
        if isinstance(args.mode, Check):
            check(client, args.mode.output)
        elif isinstance(args.mode, Screenshot):
            screenshot(client, args.mode.output, args.mode.window)
        elif isinstance(args.mode, BrowserResponse):
            mode = args.mode
            if (mode.kind == 'clear') != (mode.path is None):
                raise ValueError('clear takes no path; other response faults require --path')
            if (mode.kind == 'delay') != (mode.milliseconds is not None):
                raise ValueError('--milliseconds is required only for delay')
            payload: dict[str, str | int] = {'kind': mode.kind}
            if mode.path is not None:
                payload['path'] = mode.path
            if mode.milliseconds is not None:
                payload['milliseconds'] = mode.milliseconds
            status, result = client.request('api/browser-response', payload)
            print(json.dumps({'http_status': status, 'response': result}, indent=2))
            return 0 if status == 200 else 1
        else:
            mode = args.mode
            if mode.operation == 'screenshot':
                raise ValueError('Use screenshot --output FILE.png to save a capture')
            argument = json.loads(mode.argument) if mode.argument is not None else None
            if mode.run_in_background:
                if mode.operation not in ('start', 'action', 'sequence'):
                    raise ValueError('--run-in-background applies only to start, action, or sequence')
                argument = {'argument': argument, 'run_in_background': True}
            status, result = client.command(mode.operation, argument, request_id=mode.request_id, window=mode.window)
            print(json.dumps({'http_status': status, 'response': result}, indent=2))
            return 0 if status == 200 else 1
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == '__main__':
    sys.exit(main())
