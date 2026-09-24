"""Exercise an already-running HASS demo through its opt-in HTTP control API."""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

TOKEN_ENV = 'GARMIN_TOOLKIT_CONTROL_TOKEN'


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Do not forward the control credential to a redirect target."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def read_token(pid: int | None) -> str:
    if pid is None:
        token = os.environ.get(TOKEN_ENV, '')
    else:
        process = Path('/proc') / str(pid)
        args = (process / 'cmdline').read_bytes().split(b'\0')
        if b'--control-server' not in args or 'garmin-hass' not in Path(os.fsdecode(args[0])).name:
            raise ValueError('Token PID must identify a HASS control server')
        prefix = TOKEN_ENV.encode() + b'='
        token = next(
            (
                entry[len(prefix) :].decode()
                for entry in (process / 'environ').read_bytes().split(b'\0')
                if entry.startswith(prefix)
            ),
            '',
        )
    if not token:
        raise ValueError(f'Set {TOKEN_ENV} or supply --token-pid for a local Linux server')
    return token


class Client:
    def __init__(self, url: str, token: str):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.query or parsed.fragment:
            raise ValueError('URL must be an HTTP(S) application base URL without query or fragment')
        self.url = url.rstrip('/') + '/'
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect())
        self.session = ''
        self.request_id = 0

    def request(self, path: str, body=None, *, auth=True, headers=None) -> tuple[int, Any]:
        outgoing = {'Authorization': f'Bearer {self.token}'} if auth else {}
        outgoing.update(headers or {})
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
            raw = response.read().decode()
            try:
                value = json.loads(raw)
            except ValueError:
                value = raw
            return response.code, value

    def select(self, session: str):
        _, result = expect(self.request('api/control/sessions'), 200)
        match = next((item for item in result['sessions'] if item['id'] == session), None)
        if match is None:
            raise ValueError('Selected session is absent; discover sessions again and choose explicitly')
        self.session = session
        self.request_id = match['last_request_id']

    def command(self, operation: str, argument=None) -> tuple[int, Any]:
        self.request_id += 1
        return self.request(
            'api/control',
            {
                'session': self.session,
                'request_id': self.request_id,
                'operation': operation,
                'argument': argument,
            },
        )


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
        raise ValueError('Selected session already has an active workload')
    for path in ('api/capabilities', 'api/control/sessions'):
        expect(client.request(path, auth=False), 401)
        expect(client.request(path, headers={'Authorization': 'Bearer invalid'}), 401)
        expect(client.request(path, headers={'Origin': client.url}), 401)
        expect(client.request(path), 200)
    expect(client.request('api/control', {'session': 'unknown', 'request_id': 1, 'operation': 'status'}), 404)
    expect(client.command('unsupported'), 400)
    expect(client.command('start', 'x' * 17_000), 413)
    print('Authentication, unknown session, invalid operation, and size limit: passed', flush=True)

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
                        'session': client.session,
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
    parser.add_argument('--token-pid', type=int, help='Read the token privately from this local Linux HASS process')
    parser.add_argument('--session', help='Explicit root browser session ID from sessions')
    commands = parser.add_subparsers(dest='mode', required=True)
    commands.add_parser('sessions')
    command = commands.add_parser('command')
    command.add_argument('operation')
    command.add_argument('--argument', type=json.loads, help='JSON argument, including quotes for strings')
    command.add_argument('--request-id', type=int, help='Explicit ID for replay and stale-session checks')
    checks = commands.add_parser('check')
    checks.add_argument('--output', type=Path, default=Path('.tmp/hass-control-runtime'))
    args = parser.parse_args()
    try:
        client = Client(args.url, read_token(args.token_pid))
        if args.mode == 'sessions':
            _, result = expect(client.request('api/control/sessions'), 200)
            print(json.dumps(result, indent=2))
        else:
            if not args.session:
                parser.error('--session is required; choose explicitly from sessions')
            if args.mode == 'command' and args.request_id is not None:
                client.session, client.request_id = args.session, args.request_id - 1
            else:
                client.select(args.session)
            if args.mode == 'check':
                check(client, args.output)
            else:
                status, result = client.command(args.operation, args.argument)
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
