"""Exercise an already-running HASS demo through its opt-in HTTP control API."""

import argparse
import ipaddress
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

MAX_CAPTURE_BYTES = 8 * 1024 * 1024


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Keep control requests on the explicitly selected localhost endpoint."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Client:
    def __init__(self, url: str):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.query or parsed.fragment:
            raise ValueError('URL must be an HTTP(S) application base URL without query or fragment')
        if parsed.hostname != 'localhost' and not ipaddress.ip_address(parsed.hostname).is_loopback:
            raise ValueError('Control URL must use localhost or a loopback IP address')
        self.url = url.rstrip('/') + '/'
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        self.request_id = 0

    def request(self, path: str, body=None, *, headers=None) -> tuple[int, Any]:
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
                received_id = int(response.headers['x-garmin-request-id'])
                if isinstance(body, dict) and body.get('request_id') is not None and received_id != body['request_id']:
                    raise ValueError('Screenshot came from a different request')
                self.request_id = received_id
                validate_png(raw_bytes, metadata)
                return response.code, {'png': raw_bytes, 'metadata': metadata}
            raw = raw_bytes.decode()
            try:
                value = json.loads(raw)
                if isinstance(value, dict) and 'request_id' in value:
                    self.request_id = value['request_id']
            except ValueError:
                value = raw
            return response.code, value

    def command(self, operation: str, argument=None, *, request_id=None) -> tuple[int, Any]:
        return self.request(
            'api/control',
            {
                'request_id': request_id,
                'operation': operation,
                'argument': argument,
            },
        )


def validate_png(png: bytes, metadata: dict):
    if len(png) < 24 or png[:8] != b'\x89PNG\r\n\x1a\n' or png[12:16] != b'IHDR':
        raise ValueError('Screenshot is not a PNG')
    width, height = int.from_bytes(png[16:20]), int.from_bytes(png[20:24])
    if width != metadata['width'] or height != metadata['height'] or not 0 < width * height <= 8 * 1024 * 1024:
        raise ValueError('Screenshot dimensions do not match its metadata')


def screenshot(client: Client, output: Path):
    if output.suffix.lower() != '.png':
        raise ValueError('Screenshot output must use the .png extension')
    _, reply = expect(client.command('screenshot'), 200)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(reply['png'])
    output.with_suffix('.json').write_text(json.dumps(reply['metadata'], indent=2) + '\n')
    print(f'Saved screenshot: {output}', flush=True)


def expect(reply: tuple[int, Any], status: int) -> tuple[int, Any]:
    if reply[0] != status:
        raise ValueError(f'Expected HTTP {status}, got {reply[0]}: {reply[1]}')
    return reply


def finish(client: Client, output: Path, name: str, expected='passed', timeout=40):
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


def check(client: Client, output: Path):
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://127.0.0.1:8099/')
    commands = parser.add_subparsers(dest='mode', required=True)
    command = commands.add_parser('command')
    command.add_argument('operation')
    command.add_argument('--argument', type=json.loads, help='JSON argument, including quotes for strings')
    command.add_argument('--request-id', type=int, help='Optional explicit ID for replay checks')
    checks = commands.add_parser('check')
    checks.add_argument('--output', type=Path, default=Path('.tmp/hass-control-runtime'))
    capture = commands.add_parser('screenshot')
    capture.add_argument('--output', type=Path, default=Path('.tmp/hass-control-runtime/screenshot.png'))
    args = parser.parse_args()
    try:
        client = Client(args.url)
        if args.mode == 'check':
            check(client, args.output)
        elif args.mode == 'screenshot':
            screenshot(client, args.output)
        else:
            if args.operation == 'screenshot':
                parser.error('Use screenshot --output FILE.png to save a capture')
            status, result = client.command(args.operation, args.argument, request_id=args.request_id)
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
