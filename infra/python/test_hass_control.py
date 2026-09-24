"""Binary capture handling in the maintained HASS control client."""

import io
import json
import tempfile
import unittest
from email.message import Message
from pathlib import Path
from unittest.mock import Mock, patch

from hass_control import Client, main, screenshot


class Response(io.BytesIO):
    code = 200

    def __init__(self, content):
        super().__init__(content)
        self.headers = Message()


class CaptureTests(unittest.TestCase):
    def response(self, request_id='1', width=2):
        png = b'\x89PNG\r\n\x1a\n\0\0\0\rIHDR' + (2).to_bytes(4) + (3).to_bytes(4)
        reply = Response(png)
        reply.headers['Content-Type'] = 'image/png'
        reply.headers['x-garmin-request-id'] = request_id
        reply.headers['x-garmin-capture'] = json.dumps({'width': width, 'height': 3})
        return reply

    def client(self, response):
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
        request = client.opener.open.call_args.args[0]
        self.assertEqual(request.full_url, 'http://localhost/prefix/api/control')
        self.assertIsNone(request.get_header('Authorization'))
        self.assertEqual(json.loads(request.data)['operation'], 'screenshot')
        self.assertNotIn('session', json.loads(request.data))
        self.assertIsNone(json.loads(request.data)['request_id'])
        self.assertEqual(client.request_id, 1)

    def test_rejects_wrong_request_and_dimensions(self):
        for fields in ({'request_id': '2'}, {'width': 8}):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                self.client(self.response(**fields)).command('screenshot', request_id=1)

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
