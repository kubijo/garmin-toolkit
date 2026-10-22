"""Check the generated store tree with the real filesystem prescription."""

import os
import shutil
import socket
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import cast
from unittest.mock import patch
from urllib.parse import urlsplit

import yaml

from haos_recovery import Recovery
from haos_repository import local_store, publish_version, serve_repository
from haos_withdrawal import Withdrawal
from hass_release_contract import Architecture
from hass_repository import Repository, copy_repository, generate_repository
from json_data import object_value, read_json, text

ROOT = Path(__file__).resolve().parents[2]


class RepositoryTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix='repository-tests.', dir=ROOT / '.tmp')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / 'source'
        self.canonical = self.source / 'apps/garmin-hass/addon'
        shutil.copytree(ROOT / 'apps/garmin-hass/addon', self.canonical)
        shutil.copyfile(ROOT / 'Cargo.toml', self.source / 'Cargo.toml')
        self.branding = self.root / 'branding'
        self.branding.mkdir()
        for name in ('icon-128.png', 'logo-250x100.png'):
            (self.branding / name).write_bytes(b'synthetic test image: ' + name.encode())
        self.output = self.root / 'repository'

    def test_readonly_nix_store_metadata_is_copied_to_writable_scratch(self) -> None:
        generate_repository(self.options())
        for path in (self.output, *self.output.rglob('*')):
            path.chmod(0o555 if path.is_dir() else 0o444)
        destination = self.root / 'writable'
        copy_repository(self.output, destination)
        (destination / 'garmin_toolkit/config.yaml').write_text('version: fixture\n')
        (destination / '.git').mkdir()
        self.assertEqual((self.output / 'garmin_toolkit/config.yaml').stat().st_mode & 0o777, 0o444)
        for path in (self.output, *self.output.rglob('*')):
            path.chmod(0o755 if path.is_dir() else 0o644)

    def options(self, architectures: tuple[str, ...] = ('amd64',)) -> Repository:
        # Invalid architecture values exercise the runtime boundary as well as Tyro's typed CLI.
        return Repository(
            source=self.source,
            branding=self.branding,
            output=self.output,
            architectures=cast(tuple[Architecture, ...], architectures),
        )

    def test_generation_preserves_canonical_content_and_selects_only_requested_architectures(self) -> None:
        original = (self.canonical / 'config.yaml').read_bytes()
        generate_repository(self.options())
        actual = yaml.safe_load((self.output / 'garmin_toolkit/config.yaml').read_text())
        expected = yaml.safe_load(original)
        expected['arch'] = ['amd64']
        self.assertEqual(actual, expected)
        self.assertEqual((self.canonical / 'config.yaml').read_bytes(), original)
        for name in ('README.md', 'DOCS.md', 'CHANGELOG.md'):
            self.assertEqual((self.output / 'garmin_toolkit' / name).read_bytes(), (self.canonical / name).read_bytes())
        self.assertEqual(
            (self.output / 'garmin_toolkit/icon.png').read_bytes(), (self.branding / 'icon-128.png').read_bytes()
        )
        self.assertEqual(len(list(self.output.rglob('*'))), 8)

    def test_empty_duplicate_and_unsupported_architectures_leave_no_output(self) -> None:
        for architectures in ((), ('amd64', 'amd64'), ('riscv64',)):
            with self.subTest(architectures=architectures), self.assertRaisesRegex(ValueError, 'architectures'):
                generate_repository(self.options(architectures))
            self.assertFalse(self.output.exists())

    def test_missing_or_symlinked_inputs_leave_no_partial_tree(self) -> None:
        logo = self.branding / 'logo-250x100.png'
        original = logo.read_bytes()
        logo.unlink()
        for linked in (False, True):
            if linked:
                logo.symlink_to(self.branding / 'icon-128.png')
            with self.subTest(linked=linked), self.assertRaises((ValueError, FileNotFoundError)):
                generate_repository(self.options())
            self.assertFalse(self.output.exists())
            self.assertEqual(list(self.root.glob('.hass-repository.*')), [])
        logo.unlink()
        logo.write_bytes(original)

    def test_version_drift_and_existing_outputs_are_rejected(self) -> None:
        generate_repository(self.options())
        original = (self.output / 'garmin_toolkit/config.yaml').read_bytes()
        with self.assertRaisesRegex(ValueError, 'already exists'):
            generate_repository(self.options())
        self.assertEqual((self.output / 'garmin_toolkit/config.yaml').read_bytes(), original)
        shutil.rmtree(self.output)
        (self.source / 'Cargo.toml').write_text('[workspace.package]\nversion = "999.0.0"\n')
        with self.assertRaisesRegex(ValueError, 'version differ'):
            generate_repository(self.options())
        self.assertFalse(self.output.exists())

    def test_candidate_is_cloneable_on_its_branch_and_listener_closes_after_failure(self) -> None:
        generate_repository(self.options())
        runtime = self.root / 'runtime'
        runtime.mkdir()
        original = (self.output / 'garmin_toolkit/config.yaml').read_bytes()
        version = text(object_value(yaml.safe_load(original))['version'])
        port = 0
        with (
            self.assertRaisesRegex(RuntimeError, 'simulated VM failure'),
            serve_repository(
                self.output,
                runtime,
                image='localhost:5000/candidate',
                architecture='amd64',
                version=version,
                initial_version='0.0.0',
            ) as url,
        ):
            parsed = urlsplit(url)
            port = parsed.port or 0
            self.assertEqual(parsed.hostname, '10.0.2.2')
            self.assertEqual(parsed.fragment, 'hass-apps')
            clone_url = parsed._replace(netloc=f'127.0.0.1:{port}', fragment='').geturl()
            cloned = self.root / 'cloned'
            git_environment = {**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'}
            subprocess.run(
                [
                    'git',
                    '-c',
                    'http.proxy=',
                    'clone',
                    '-q',
                    '--depth=1',
                    '--recursive',
                    '--shallow-submodules',
                    '--branch',
                    'hass-apps',
                    clone_url,
                    str(cloned),
                ],
                env=git_environment,
                check=True,
                capture_output=True,
                text=True,
                timeout=15,
            )
            config = yaml.safe_load((cloned / 'garmin_toolkit/config.yaml').read_bytes())
            self.assertEqual(config['image'], 'localhost:5000/candidate')
            self.assertEqual(config['arch'], ['amd64'])
            self.assertEqual(config['version'], '0.0.0')
            publish_version(runtime, version)
            subprocess.run(
                ['git', '-C', str(cloned), 'pull', '--ff-only', '-q'],
                env=git_environment,
                check=True,
                capture_output=True,
                timeout=15,
            )
            self.assertEqual(yaml.safe_load((cloned / 'garmin_toolkit/config.yaml').read_bytes())['version'], version)
            raise RuntimeError('simulated VM failure')
        self.assertEqual((self.output / 'garmin_toolkit/config.yaml').read_bytes(), original)
        with socket.socket() as probe:
            probe.settimeout(1)
            self.assertNotEqual(probe.connect_ex(('127.0.0.1', port)), 0)

    def test_withdrawal_and_replacement_reach_a_real_git_http_consumer_without_publication_access(self) -> None:
        generate_repository(self.options())
        original = (self.output / 'garmin_toolkit/config.yaml').read_bytes()
        version = text(object_value(yaml.safe_load(original))['version'])
        runtime, evidence = self.root / 'runtime', self.root / 'evidence'
        runtime.mkdir()
        evidence.mkdir()
        with (
            serve_repository(
                self.output,
                runtime,
                image='localhost:5000/candidate',
                architecture='amd64',
                version=version,
                initial_version='0.0.0',
            ) as url,
            patch('hass_publish.command', side_effect=AssertionError('Public command invoked')),
            patch('hass_publish.store', side_effect=AssertionError('Public store accessed')),
        ):
            parsed = urlsplit(url)
            clone_url = parsed._replace(netloc=f'127.0.0.1:{parsed.port}', fragment='').geturl()
            cloned = self.root / 'consumer'
            environment = {**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'}

            def consumer(*arguments: str) -> None:
                subprocess.run(
                    ['git', '-c', 'http.proxy=', *arguments],
                    env=environment,
                    check=True,
                    capture_output=True,
                    timeout=15,
                )

            consumer('clone', '-q', '--depth=1', '--branch', 'hass-apps', clone_url, str(cloned))
            fixture = Withdrawal(runtime, evidence)
            fixture.withdraw('0.0.0')
            consumer('-C', str(cloned), 'pull', '--ff-only', '-q')
            self.assertFalse((cloned / 'garmin_toolkit').exists())
            self.assertEqual((cloned / 'repository.yaml').read_bytes(), (self.output / 'repository.yaml').read_bytes())
            with self.assertRaisesRegex(ValueError, 'greater numeric'):
                fixture.replace('0.0.0')
            fixture.replace(version)
            consumer('-C', str(cloned), 'pull', '--ff-only', '-q')
            replaced = object_value(yaml.safe_load((cloned / 'garmin_toolkit/config.yaml').read_bytes()))
            self.assertEqual(replaced['version'], version)
            self.assertEqual(replaced['image'], 'localhost:5000/candidate')
            fixture.qualify()
            self.assertIs(
                object_value(read_json(evidence / 'withdrawal-replacement/promotion.json'))['qualified'], True
            )
        self.assertEqual((self.output / 'garmin_toolkit/config.yaml').read_bytes(), original)

    def test_withdrawal_refuses_a_repository_symlink_outside_its_owned_runtime(self) -> None:
        runtime = self.root / 'runtime'
        (runtime / 'repository-http').mkdir(parents=True)
        other = self.root / 'other.git'
        other.mkdir()
        (runtime / 'repository-http/garmin-toolkit.git').symlink_to(other, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'owned local Git repository'):
            Withdrawal(runtime, self.root).store()

    def test_failed_acceptance_rolls_back_the_exact_private_head_and_preserves_qualified_retry(self) -> None:
        generate_repository(self.options())
        original = (self.output / 'garmin_toolkit/config.yaml').read_bytes()
        version = text(object_value(yaml.safe_load(original))['version'])
        runtime, evidence = self.root / 'runtime', self.root / 'evidence'
        runtime.mkdir()
        evidence.mkdir()
        with (
            serve_repository(
                self.output,
                runtime,
                image='localhost:5000/candidate',
                architecture='amd64',
                version=version,
                initial_version='0.0.0',
            ) as url,
            patch('hass_publish.command', side_effect=AssertionError('Public command invoked')),
            patch('hass_publish.store', side_effect=AssertionError('Public store accessed')),
        ):
            parsed = urlsplit(url)
            clone_url = parsed._replace(netloc=f'127.0.0.1:{parsed.port}', fragment='').geturl()
            environment = {**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'}

            def discovered(name: str) -> str:
                cloned = self.root / name
                subprocess.run(
                    [
                        'git',
                        '-c',
                        'http.proxy=',
                        'clone',
                        '-q',
                        '--depth=1',
                        '--branch',
                        'hass-apps',
                        clone_url,
                        str(cloned),
                    ],
                    env=environment,
                    check=True,
                    capture_output=True,
                    timeout=15,
                )
                return text(
                    object_value(yaml.safe_load((cloned / 'garmin_toolkit/config.yaml').read_bytes()))['version']
                )

            fixture = Recovery(runtime, evidence)
            owner = local_store(runtime)
            before = owner.head(owner.branch)
            fixture.prepare(version)
            self.assertEqual(discovered('baseline-consumer'), '0.0.0')
            with self.assertRaises(FileNotFoundError):
                fixture.advertise('retry')
            fixture.advertise('failed')
            self.assertEqual(discovered('failed-consumer'), version)
            fixture.rollback()
            self.assertEqual(owner.head(owner.branch), before)
            self.assertEqual(discovered('recovered-consumer'), '0.0.0')
            fixture.advertise('retry')
            self.assertEqual(discovered('retry-consumer'), version)
            result = fixture.qualify()
            self.assertIs(result['retry_qualified'], True)
            self.assertIs(result['qualified_retry_preserved'], True)
        self.assertEqual((self.output / 'garmin_toolkit/config.yaml').read_bytes(), original)


if __name__ == '__main__':
    unittest.main()
