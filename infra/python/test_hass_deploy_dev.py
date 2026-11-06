"""The SSH deployment must not touch HA before a complete local bundle exists."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import hass_deploy_dev as dev


class DeployTests(unittest.TestCase):
    def test_build_failure_never_connects_to_host(self) -> None:
        with (
            patch.object(dev.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'nix')) as run,
            self.assertRaises(subprocess.CalledProcessError),
        ):
            dev.deploy(dev.Options(host='ha-test'))
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0][0], 'nix')

    def test_build_only_never_connects_to_host(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'bundle.tar.gz'
            archive.touch()
            with patch.object(
                dev.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, str(archive))
            ) as run:
                dev.deploy(dev.Options(host='ha-test', build_only=True))
            self.assertEqual(run.call_count, 1)

    def test_remote_failure_cleans_up_and_propagates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'bundle.tar.gz'
            archive.touch()
            (Path(directory) / 'Cargo.toml').write_text('[workspace.package]\nversion = "9.8.7"\n')
            with (
                patch.object(dev, 'ROOT', Path(directory)),
                patch.object(dev.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, str(archive))),
                patch.object(dev.Installer, 'install', side_effect=ValueError('SSH failed')) as install,
                self.assertRaisesRegex(ValueError, 'SSH failed'),
            ):
                dev.deploy(dev.Options(host='ha-test'))
            install.assert_called_once_with(archive, '/local_apps', 'af63a031_garmin_toolkit', '9.8.7-dev')


if __name__ == '__main__':
    unittest.main()
