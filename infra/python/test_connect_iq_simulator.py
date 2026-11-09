import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import connect_iq as local
import connect_iq_simulator as simulator


@unittest.skipUnless(sys.platform == 'linux', 'The owned SDK simulator runtime is Linux-only')
class SimulatorTests(unittest.TestCase):
    def settings(self, root: Path) -> None:
        state = root / '.tmp/connect-iq'
        state.mkdir(parents=True)
        (state / 'local.json').write_text(json.dumps({'sdk': str(state / 'sdk'), 'key': str(state / 'key.der')}))

    def test_owned_runs_are_exclusive_and_release_their_process_and_lock(self) -> None:
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(local, 'ROOT', Path(temporary)),
            patch.object(local, 'doctor'),
            patch.object(simulator, 'simulator_ready', side_effect=[False, False, True]),
            patch.object(simulator, 'Runtime') as runtime,
            patch.object(simulator.subprocess, 'Popen') as process,
        ):
            self.settings(Path(temporary))
            runtime.return_value.container.return_value.__enter__.return_value = ['owned-simulator']
            process.return_value.poll.return_value = None
            with simulator.session('test') as active:
                self.assertTrue(active.output.is_dir())
                with self.assertRaisesRegex(ValueError, 'Another owned simulator'), simulator.session('preview'):
                    self.fail('A second simulator must not launch')
            self.assertEqual(process.call_count, 1)
            process.return_value.terminate.assert_called_once()
            process.return_value.wait.assert_called_once_with(timeout=10)
            runtime.return_value.container.return_value.__exit__.assert_called_once()
            # A completed run must not leave a stale lock behind.
            with (
                patch.object(simulator, 'simulator_ready', side_effect=[False, False, True]),
                simulator.session('preview'),
            ):
                pass
            self.assertEqual(process.call_count, 2)

    def test_an_external_simulator_is_never_replaced(self) -> None:
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(local, 'ROOT', Path(temporary)),
            patch.object(simulator, 'simulator_ready', return_value=True),
            patch.object(simulator, 'Runtime') as runtime,
            patch.object(simulator.subprocess, 'Popen') as process,
        ):
            with self.assertRaisesRegex(ValueError, 'port 1234 is occupied'), simulator.session('test'):
                self.fail('An external simulator must not be reused')
            runtime.assert_not_called()
            process.assert_not_called()

    def test_failed_app_or_tests_still_stop_the_owned_simulator(self) -> None:
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(local, 'ROOT', Path(temporary)),
            patch.object(local, 'doctor'),
            patch.object(simulator, 'simulator_ready', side_effect=[False, False, True]),
            patch.object(simulator, 'Runtime') as runtime,
            patch.object(simulator.subprocess, 'Popen') as process,
        ):
            self.settings(Path(temporary))
            runtime.return_value.container.return_value.__enter__.return_value = ['owned-simulator']
            process.return_value.poll.return_value = None
            with self.assertRaisesRegex(RuntimeError, 'synthetic app failure'), simulator.session('preview'):
                raise RuntimeError('synthetic app failure')
            process.return_value.terminate.assert_called_once()
            runtime.return_value.container.return_value.__exit__.assert_called_once()
