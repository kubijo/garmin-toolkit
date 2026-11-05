"""Exercise QMP framing and cleanup without running QEMU or inspecting host devices."""

import json
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from desktop_vm_qmp import Qmp
from json_data import Object


def channel(replies: list[Object]) -> tuple[Mock, Mock]:
    connection, stream = Mock(), Mock()
    connection.makefile.return_value = stream
    stream.readline.side_effect = [json.dumps(reply).encode() + b'\n' for reply in replies]
    return connection, stream


class QmpTests(unittest.TestCase):
    def test_events_do_not_replace_command_replies(self) -> None:
        connection, stream = channel(
            [
                {'QMP': {}},
                {'event': 'HELLO'},
                {'return': {}},
                {'event': 'RESUME'},
                {'return': {'status': 'running'}},
            ]
        )
        with patch('desktop_vm_qmp.socket.socket', return_value=connection):
            client = Qmp(Path('/synthetic.sock'))
            try:
                self.assertEqual(client.command('query-status'), {'status': 'running'})
            finally:
                client.close()
        self.assertEqual(json.loads(stream.write.call_args_list[0].args[0])['execute'], 'qmp_capabilities')
        self.assertEqual(json.loads(stream.write.call_args_list[1].args[0])['execute'], 'query-status')
        connection.close.assert_called_once()
        stream.close.assert_called_once()

    def test_handshake_failure_closes_both_owned_resources(self) -> None:
        connection, stream = channel([{'QMP': {}}, {'error': {'desc': 'capabilities rejected'}}])
        with (
            patch('desktop_vm_qmp.socket.socket', return_value=connection),
            self.assertRaisesRegex(ValueError, 'rejected'),
        ):
            Qmp(Path('/synthetic.sock'))
        connection.close.assert_called_once()
        stream.close.assert_called_once()

    def test_command_errors_are_reported(self) -> None:
        connection, _ = channel([{'QMP': {}}, {'return': {}}, {'error': {'desc': 'USB device busy'}}])
        with patch('desktop_vm_qmp.socket.socket', return_value=connection):
            client = Qmp(Path('/synthetic.sock'))
            try:
                with self.assertRaisesRegex(ValueError, 'USB device busy'):
                    client.command('device_add')
            finally:
                client.close()

    def test_missing_response_cannot_be_success(self) -> None:
        connection, stream = channel([{'QMP': {}}, {'return': {}}])
        with patch('desktop_vm_qmp.socket.socket', return_value=connection):
            client = Qmp(Path('/synthetic.sock'))
            stream.readline.side_effect = [b'']
            try:
                with self.assertRaisesRegex(ValueError, 'Missing'):
                    client.command('query-status')
            finally:
                client.close()


if __name__ == '__main__':
    unittest.main()
