"""Keep published-baseline admission bound to authenticated assets and the selected source tag."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import haos_upgrade as upgrade
from fixtures.images import write_image
from haos_platform import PLATFORMS, inspect_archive
from json_data import Object, file_sha256, json_bytes, object_rows


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
