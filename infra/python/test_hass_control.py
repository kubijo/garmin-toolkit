"""Binary capture handling in the maintained HASS control client."""

import io
import json
import tempfile
import unittest
from email.message import Message
from pathlib import Path
from typing import cast
from unittest.mock import Mock, patch

from hass_control import Client, main, screenshot


class Response(io.BytesIO):
    code = 200

    def __init__(self, content: bytes) -> None:
        super().__init__(content)
        self.headers = Message()


class BackgroundRunTests(unittest.TestCase):
    def test_cli_sends_background_option_with_launch_and_never_configures_the_session(self):
        client = Mock()
        client.command.return_value = (200, {'value': None})
        for operation, argument in (
            ('start', 'activity-smoke'),
            ('action', {'kind': 'click', 'target': 'profile.0'}),
            ('sequence', [{'kind': 'click', 'target': 'profile.0'}]),
        ):
            client.reset_mock()
            with (
                patch(
                    'sys.argv',
                    [
                        'hass_control.py',
                        'command',
                        operation,
                        '--argument',
                        json.dumps(argument),
                        '--run-in-background',
                    ],
                ),
                patch('hass_control.Client', return_value=client),
                patch('sys.stdout', new_callable=io.StringIO),
            ):
                self.assertEqual(main(), 0)
            client.command.assert_called_once_with(
                operation, {'argument': argument, 'run_in_background': True}, request_id=None, window=None
            )


class CaptureTests(unittest.TestCase):
    def response(self, request_id: str = '1', width: int = 2) -> Response:
        png = b'\x89PNG\r\n\x1a\n\0\0\0\rIHDR' + (2).to_bytes(4) + (3).to_bytes(4)
        reply = Response(png)
        reply.headers['Content-Type'] = 'image/png'
        reply.headers['x-garmin-request-id'] = request_id
        reply.headers['x-garmin-capture'] = json.dumps({'width': width, 'height': 3})
        return reply

    def client(self, response: Response) -> Client:
        client = Client('http://localhost/prefix/')
        client.opener = Mock()
        client.opener.open.return_value = response
        return client

    def test_saves_binary_capture_and_metadata(self):
        client = self.client(self.response())
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'capture.png'
            screenshot(client, output)
            self.assertEqual(output.read_bytes()[:8], b'\x89PNG\r\n\x1a\n')
            self.assertEqual(json.loads(output.with_suffix('.json').read_text())['width'], 2)
        request = cast(Mock, client.opener).open.call_args.args[0]
        self.assertEqual(request.full_url, 'http://localhost/prefix/api/control')
        self.assertIsNone(request.get_header('Authorization'))
        self.assertEqual(json.loads(request.data)['operation'], 'screenshot')
        self.assertNotIn('session', json.loads(request.data))
        self.assertIsNone(json.loads(request.data)['request_id'])
        self.assertEqual(client.request_id, 1)

    def test_rejects_wrong_request_and_dimensions(self):
        for request_id, width in [('2', 2), ('1', 8)]:
            with self.subTest(request_id=request_id, width=width), self.assertRaises(ValueError):
                self.client(self.response(request_id=request_id, width=width)).command('screenshot', request_id=1)

    def test_child_capture_keeps_the_explicit_window_handle(self):
        client = self.client(self.response())
        with tempfile.TemporaryDirectory() as directory:
            screenshot(client, Path(directory) / 'child.png', 'files:12')
        request = cast(Mock, client.opener).open.call_args.args[0]
        self.assertEqual(json.loads(request.data)['window'], 'files:12')

    def test_native_capture_without_broker_request_id(self):
        response = self.response()
        del response.headers['x-garmin-request-id']
        status, result = self.client(response).command('screenshot')
        self.assertEqual(status, 200)
        self.assertEqual(result['metadata']['width'], 2)

    def test_screenshot_cli_needs_no_token_environment_or_session(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'capture.png'
            with (
                patch.dict('os.environ', {}, clear=True),
                patch('sys.argv', ['hass_control.py', 'screenshot', '--output', str(output)]),
                patch('hass_control.Client', return_value=self.client(self.response())),
            ):
                self.assertEqual(main(), 0)
            self.assertTrue(output.is_file())

    def test_only_loopback_urls_are_accepted(self):
        for url in ('http://192.168.1.3/', 'https://example.com/'):
            with self.subTest(url=url), self.assertRaises(ValueError):
                Client(url)
        for url in ('http://127.0.0.1/', 'http://localhost/', 'http://[::1]/'):
            Client(url)


if __name__ == '__main__':
    unittest.main()
