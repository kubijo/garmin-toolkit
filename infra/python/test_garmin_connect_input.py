"""Contract and integrity rejection checks for descriptor updates."""

import hashlib
import json
import unittest

from garmin_connect_input import DIRECTORY, verify


class InputTests(unittest.TestCase):
    def test_pinned_artifact(self) -> None:
        pin = json.loads((DIRECTORY / 'input.json').read_text())
        verify((DIRECTORY / 'openapi.json').read_bytes(), pin['sha256'])

    def test_tampering_is_rejected(self) -> None:
        data = (DIRECTORY / 'openapi.json').read_bytes()
        with self.assertRaisesRegex(ValueError, 'SHA-256'):
            verify(data + b' ', hashlib.sha256(data).hexdigest())

    def test_wrong_document_and_missing_operation_are_rejected(self) -> None:
        for field in ('title', 'path', 'profile'):
            spec = json.loads((DIRECTORY / 'openapi.json').read_bytes())
            if field == 'title':
                spec['info']['title'] = 'Different API'
            elif field == 'path':
                del spec['paths']['/download-service/files/activity/{activityId}']
            else:
                del spec['paths']['/userprofile-service/userprofile/settings']
            data = json.dumps(spec).encode()
            with self.assertRaises(ValueError):
                verify(data, hashlib.sha256(data).hexdigest())


if __name__ == '__main__':
    unittest.main()
