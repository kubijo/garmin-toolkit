"""Keep published-baseline admission bound to authenticated assets and the selected source tag."""

import subprocess
import tempfile
import unittest
from copy import deepcopy
from pathlib import Path
from typing import cast
from unittest.mock import patch

import haos_upgrade as upgrade
from fixtures.images import write_image
from haos_platform import PLATFORMS, inspect_archive
from hass_release_contract import Platform as NativePlatform
from json_data import Object, file_sha256, json_bytes, object_rows


class UpgradeCoverageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.platform: NativePlatform = {
            'architecture': 'aarch64',
            'config_digest': 'sha256:' + 'a' * 64,
            'manifest_digest': 'sha256:' + 'b' * 64,
            'archive_sha256': 'c' * 64,
            'sbom_sha256': 'd' * 64,
            'builder_system': 'aarch64-linux',
            'derivation': '/nix/store/' + 'a' * 32 + '-image.drv',
            'workflow_run': None,
        }
        self.baseline: Object = {
            'architecture': 'aarch64',
            'release': 'v0.1.3',
            'version': '0.1.3',
            'source_commit': '1' * 40,
            'archive_sha256': '2' * 64,
            'manifest_digest': 'sha256:' + '3' * 64,
            'config_digest': 'sha256:' + '4' * 64,
            'registry_index_digest': 'sha256:' + '5' * 64,
            'release_manifest_sha256': '6' * 64,
            'signatures_verified': True,
        }
        self.coverage: Object = {
            'baseline': 'published release v0.1.3',
            'published_baseline': self.baseline,
            'candidate_version': '0.1.4',
            'candidate_archive_version': '0.1.4',
            'migration_compatibility': True,
        }
        self.source: Object = {'commit': '7' * 40}

    def verify(self, coverage: Object, *, version: str = '0.1.4') -> None:
        upgrade.check_upgrade_coverage(coverage, self.platform, self.source, version)

    def test_authenticated_older_release_is_required_for_each_architecture(self) -> None:
        for architecture, system in (('aarch64', 'aarch64-linux'), ('amd64', 'x86_64-linux')):
            self.baseline['architecture'] = architecture
            platform = {**self.platform, 'architecture': architecture, 'builder_system': system}
            upgrade.check_upgrade_coverage(self.coverage, cast(NativePlatform, platform), self.source, '0.1.4')

    def test_legacy_fixture_evidence_cannot_qualify_a_new_release(self) -> None:
        fixture: Object = {
            'baseline': 'metadata-only fixture; candidate filesystem; no previous public release',
            'migration_compatibility': False,
        }
        self.verify(fixture, version='0.1.3')
        with self.assertRaises(ValueError):
            self.verify(fixture)
        with self.assertRaises(ValueError):
            self.verify({**fixture, 'migration_compatibility': True}, version='0.1.3')

    def test_baseline_identity_authentication_versions_and_architecture_cannot_be_substituted(self) -> None:
        for key, value in (
            ('signatures_verified', False),
            ('architecture', 'amd64'),
            ('release', 'v0.1.2'),
            ('version', '0.1.4'),
            ('source_commit', self.source['commit']),
            ('config_digest', self.platform['config_digest']),
            ('archive_sha256', self.platform['archive_sha256']),
        ):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.verify({**self.coverage, 'published_baseline': {**self.baseline, key: value}})
        for key in self.baseline:
            identity = deepcopy(self.baseline)
            del identity[key]
            with self.subTest(missing=key), self.assertRaises(ValueError):
                self.verify({**self.coverage, 'published_baseline': identity})
        for version in ('0.1.4', '0.1.5', '01.1.3'):
            with self.subTest(version=version), self.assertRaises(ValueError):
                self.verify(
                    {
                        **self.coverage,
                        'baseline': 'published release v' + version,
                        'published_baseline': {**self.baseline, 'release': 'v' + version, 'version': version},
                    }
                )

    def test_forged_coverage_private_aliases_and_malformed_digests_are_rejected(self) -> None:
        for key, value in (
            ('candidate_version', '0.1.4-test'),
            ('candidate_archive_version', '0.1.3'),
            ('baseline', 'metadata-only fixture'),
            ('migration_compatibility', False),
        ):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.verify({**self.coverage, key: value})
        for key in (
            'source_commit',
            'archive_sha256',
            'manifest_digest',
            'config_digest',
            'registry_index_digest',
            'release_manifest_sha256',
        ):
            with self.subTest(digest=key), self.assertRaises(ValueError):
                self.verify({**self.coverage, 'published_baseline': {**self.baseline, key: 'invalid'}})


class PublishedUpgradeTests(unittest.TestCase):
    def setUp(self) -> None:
        scratch = Path(__file__).resolve().parents[2] / '.tmp'
        scratch.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='published-upgrade-tests.', dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.archive = self.root / 'baseline.oci.tar'
        write_image(self.archive, version='0.1.0')
        image = inspect_archive(self.archive)
        digest = file_sha256(self.archive)
        self.document: Object = {
            'schema_version': 1,
            'status': 'candidate',
            'version': '0.1.0',
            'source': {
                'repository': upgrade.REPOSITORY,
                'commit': '1' * 40,
                'tag': 'v0.1.0',
                'lock_sha256': '2' * 64,
            },
            'image': {'repository': upgrade.IMAGE, 'index_digest': 'sha256:' + '3' * 64},
            'platforms': [
                {
                    'architecture': 'amd64',
                    'config_digest': image.config_digest,
                    'manifest_digest': image.manifest_digest,
                    'archive_sha256': digest,
                    'sbom_sha256': '4' * 64,
                    'builder_system': 'x86_64-linux',
                    'derivation': '/nix/store/' + 'a' * 32 + '-image.drv',
                    'workflow_run': None,
                }
            ],
            'acceptance': [],
            'files': [
                {'name': name, 'role': role, 'sha256': '5' * 64, 'size': 1}
                for name, role in (
                    ('source.tar.gz', 'source'),
                    ('LICENSE', 'license'),
                    ('COPYRIGHT.md', 'copyright'),
                    ('NOTICES.md', 'notices'),
                    ('bundle.json', 'license_bundle'),
                    ('sbom.json', 'sbom'),
                    ('sources.tar.gz', 'dependency_sources'),
                    ('sources.json', 'source_inventory'),
                )
            ]
            + [
                {
                    'name': self.archive.name,
                    'role': 'image_archive',
                    'architecture': 'amd64',
                    'sha256': digest,
                    'size': self.archive.stat().st_size,
                }
            ],
        }
        self.inventory()
        self.attest = self.enterContext(patch.object(upgrade, 'attest'))
        self.release = self.enterContext(
            patch.object(
                upgrade.subprocess,
                'run',
                return_value=subprocess.CompletedProcess([], 0, stdout='{"tagName":"v0.1.0","isDraft":false}'),
            )
        )
        self.enterContext(patch.object(upgrade, 'git', return_value='1' * 40))
        self.enterContext(
            patch.object(
                upgrade, 'source_metadata', return_value=('0.1.0', {'version': '0.1.0', 'image': upgrade.IMAGE})
            )
        )

    def inventory(self) -> None:
        manifest = self.root / 'release.json'
        manifest.write_bytes(json_bytes(self.document))
        (self.root / 'SHA256SUMS').write_text(
            '\n'.join(file_sha256(path) + '  ' + path.name for path in (manifest, self.archive)) + '\n'
        )

    def verify(self) -> tuple[Path, Object]:
        return upgrade.published_baseline(self.root, self.root, PLATFORMS['amd64'])

    def test_published_image_is_authenticated_without_modifying_its_archive(self) -> None:
        before = self.archive.read_bytes()
        archive, identity = self.verify()
        self.assertEqual(archive, self.archive)
        self.assertEqual(identity['release'], 'v0.1.0')
        self.assertEqual(identity['architecture'], 'amd64')
        self.assertEqual(identity['source_commit'], '1' * 40)
        self.assertIs(identity['signatures_verified'], True)
        self.assertEqual(self.archive.read_bytes(), before)
        self.assertEqual(self.attest.call_count, 3)
        for call in self.attest.call_args_list:
            self.assertEqual(call.args[0].version, '0.1.0')
            self.assertEqual(call.args[0].commit, '1' * 40)

    def test_untrusted_checksums_cannot_reach_image_admission(self) -> None:
        self.attest.side_effect = subprocess.CalledProcessError(1, 'gh')
        with patch.object(upgrade, 'inspect_archive') as inspect, self.assertRaises(subprocess.CalledProcessError):
            self.verify()
        inspect.assert_not_called()

    def test_unpublished_draft_cannot_be_used_as_a_published_baseline(self) -> None:
        self.release.return_value.stdout = '{"tagName":"v0.1.0","isDraft":true}'
        with self.assertRaisesRegex(ValueError, 'already published'):
            self.verify()
        self.attest.assert_not_called()

    def test_altered_manifest_and_archive_are_rejected(self) -> None:
        manifest = self.root / 'release.json'
        manifest.write_bytes(manifest.read_bytes() + b'\n')
        with self.assertRaisesRegex(ValueError, 'authenticated checksums'):
            self.verify()
        self.inventory()
        original = self.archive.read_bytes()
        self.archive.write_bytes(b'X' + original[1:])
        with self.assertRaisesRegex(ValueError, 'authenticated checksums'):
            self.verify()

    def test_wrong_tag_source_and_signed_image_identity_are_rejected(self) -> None:
        with (
            patch.object(upgrade, 'git', return_value='6' * 40),
            self.assertRaisesRegex(ValueError, 'selected release tag'),
        ):
            self.verify()
        object_rows(self.document['platforms'])[0]['config_digest'] = 'sha256:' + '6' * 64
        self.inventory()
        with self.assertRaisesRegex(ValueError, 'OCI identity'):
            self.verify()


if __name__ == '__main__':
    unittest.main()
