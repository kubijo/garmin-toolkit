"""Exercise release assembly, trusted hash verification, and rejection of incomplete candidates."""

import copy
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import patch

from fixtures.images import write_image
from haos_platform import inspect_archive
from hass_release import Assemble, Verify, assemble, verify
from hass_release_contract import CONTRACT, json_bytes

SCRATCH = Path(__file__).resolve().parents[2] / '.tmp'


class ReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        SCRATCH.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='release-tests.', dir=SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        self.artifacts = self.root / 'artifacts'
        self.artifacts.mkdir()
        self.receipt = self.root / 'build.json'
        self.output = self.root / 'candidate'
        files = {
            'Cargo.toml': '[workspace.package]\nversion = "0.1.0"\n',
            'flake.lock': '{"fixture": true}\n',
            'apps/garmin-hass/addon/config.yaml': (
                'version: 0.1.0\nimage: ghcr.io/kubijo/garmin-toolkit-hass\narch: [amd64, aarch64]\n'
            ),
            'LICENSE-AGPL': 'Synthetic license fixture\n',
            'COPYRIGHT.md': 'Synthetic copyright fixture\n',
            'THIRD_PARTY_NOTICES.md': 'Synthetic notice fixture\n',
            'assets/licenses/bundle-hass.json': '{"fixture": true}\n',
            'README.md': 'Synthetic build instructions\n',
        }
        for name, value in files.items():
            path = self.source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
        self.git('init', '--initial-branch=main')
        self.git('add', '.')
        self.git('commit', '-m', 'Synthetic release fixture')
        self.git('tag', 'v0.1.0')
        self.git('checkout', '--detach', 'v0.1.0')
        self.document: dict[str, Any] = {
            'schema_version': 1,
            'version': '0.1.0',
            'source': {
                'repository': 'https://github.com/kubijo/garmin-toolkit',
                'tag': 'v0.1.0',
                'commit': self.git('rev-parse', 'HEAD'),
                'lock_sha256': hashlib.sha256(files['flake.lock'].encode()).hexdigest(),
            },
            'image': {'repository': 'ghcr.io/kubijo/garmin-toolkit-hass', 'index_digest': 'sha256:' + '1' * 64},
            'platforms': [self.image('amd64'), self.image('aarch64')],
        }
        self.save_receipt()

    def git(self, *arguments: str) -> str:
        return subprocess.check_output(
            [
                'git',
                '-C',
                str(self.source),
                '-c',
                'user.name=Release Fixture',
                '-c',
                'user.email=fixture@example.invalid',
                '-c',
                'commit.gpgsign=false',
                '-c',
                'core.hooksPath=' + str(self.root / 'no-hooks'),
                *arguments,
            ],
            env={**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'},
            stderr=subprocess.PIPE,
            text=True,
        ).strip()

    def image(
        self,
        architecture: str,
        *,
        actual: str | None = None,
        version: str = '0.1.0',
        tag: str = 'ghcr.io/kubijo/garmin-toolkit-hass:0.1.0',
    ) -> dict[str, str]:
        archive = self.artifacts / f'garmin-toolkit-hass-0.1.0-{architecture}.oci.tar'
        digest = write_image(
            archive,
            architecture=actual or ('arm64' if architecture == 'aarch64' else 'amd64'),
            hass_arch=architecture,
            version=version,
            tag=tag,
        )
        sbom = self.artifacts / f'garmin-toolkit-hass-0.1.0-{architecture}.spdx.json'
        sbom.write_text(json.dumps({'spdxVersion': 'SPDX-2.3', 'packages': [{'name': 'synthetic-image'}]}))
        return {
            'architecture': architecture,
            'manifest_digest': inspect_archive(archive).manifest_digest,
            'config_digest': digest,
            'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
            'sbom_sha256': hashlib.sha256(sbom.read_bytes()).hexdigest(),
            'builder_system': 'x86_64-linux' if architecture == 'amd64' else 'aarch64-linux',
            'derivation': '/nix/store/' + '0' * 32 + '-fixture.drv',
            'workflow_run': 'https://github.com/kubijo/garmin-toolkit/actions/runs/123',
        }

    def save_receipt(self) -> None:
        self.receipt.write_bytes(json_bytes(self.document))

    def options(self) -> Assemble:
        self.save_receipt()
        return Assemble(source=self.source, receipt=self.receipt, artifacts=self.artifacts, output=self.output)

    def verify_options(self, digest: str) -> Verify:
        return Verify(
            directory=self.output, manifest_sha256=digest, version='0.1.0', commit=self.document['source']['commit']
        )

    def commit_changes(self) -> None:
        self.git('add', '.')
        self.git('commit', '-m', 'Changed synthetic fixture')
        self.git('tag', '--force', 'v0.1.0')
        self.git('branch', '--force', 'main')
        self.document['source']['commit'] = self.git('rev-parse', 'HEAD')

    def test_assembles_both_platforms_and_passes_independent_sha256sum(self) -> None:
        digest = assemble(self.options())
        verify(self.verify_options(digest))
        checked = subprocess.run(
            ['sha256sum', '--check', 'SHA256SUMS'], cwd=self.output, capture_output=True, text=True, check=False
        )
        self.assertEqual(checked.returncode, 0, checked.stderr)
        manifest = json.loads((self.output / 'release.json').read_text())
        self.assertEqual(manifest['status'], 'candidate')
        self.assertEqual(manifest['acceptance'], [])
        self.assertEqual(len(manifest['files']), 9)
        for path in self.artifacts.iterdir():
            self.assertEqual(path.read_bytes(), (self.output / path.name).read_bytes())
        with tarfile.open(self.output / 'garmin-toolkit-0.1.0-source.tar.gz') as source:
            stream = source.extractfile('garmin-toolkit-0.1.0/Cargo.toml')
            assert stream is not None
            self.assertEqual(stream.read(), (self.source / 'Cargo.toml').read_bytes())

    def test_identical_inputs_produce_identical_manifest_and_source_archive(self) -> None:
        first = assemble(self.options())
        original = self.output
        self.output = self.root / 'second'
        self.assertEqual(assemble(self.options()), first)
        for path in original.iterdir():
            self.assertEqual(path.read_bytes(), (self.output / path.name).read_bytes(), path.name)

    def test_arm64_only_candidate_uses_home_assistant_and_oci_architecture_names(self) -> None:
        self.document['platforms'] = [self.document['platforms'][1]]
        for path in self.artifacts.glob('*-amd64.*'):
            path.unlink()
        digest = assemble(self.options())
        verify(self.verify_options(digest))
        self.assertEqual(len(json.loads((self.output / 'release.json').read_text())['files']), 7)

    def test_branch_checkout_and_dirty_or_untracked_source_are_rejected(self) -> None:
        self.git('checkout', 'main')
        with self.assertRaisesRegex(ValueError, 'detached'):
            assemble(self.options())
        self.git('checkout', '--detach', 'v0.1.0')
        for name in ('README.md', 'unexpected'):
            path = self.source / name
            previous = path.read_bytes() if path.exists() else None
            path.write_text('dirty')
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'dirty'):
                assemble(self.options())
            if previous is None:
                path.unlink()
            else:
                path.write_bytes(previous)
        self.assertFalse(self.output.exists())

    def test_source_receipt_revision_lock_and_version_must_match(self) -> None:
        original = copy.deepcopy(self.document)
        for field, value, message in (
            ('commit', 'a' * 40, 'commit'),
            ('lock_sha256', 'a' * 64, 'lockfile'),
            ('tag', 'v0.2.0', 'tag'),
        ):
            self.document = copy.deepcopy(original)
            self.document['source'][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, message):
                assemble(self.options())
        self.document = original
        (self.source / 'apps/garmin-hass/addon/config.yaml').write_text('version: 0.2.0\n')
        self.commit_changes()
        with self.assertRaisesRegex(ValueError, 'versions differ'):
            assemble(self.options())

    def test_tag_outside_main_is_rejected(self) -> None:
        (self.source / 'README.md').write_text('Unmerged fixture')
        self.git('add', '.')
        self.git('commit', '-m', 'Unmerged fixture')
        self.git('tag', '--force', 'v0.1.0')
        self.document['source']['commit'] = self.git('rev-parse', 'HEAD')
        with self.assertRaisesRegex(ValueError, 'reachable from main'):
            assemble(self.options())

    def test_committed_source_blobs_win_over_assume_unchanged_worktree_content(self) -> None:
        original = (self.source / 'LICENSE-AGPL').read_bytes()
        self.git('update-index', '--assume-unchanged', 'LICENSE-AGPL')
        (self.source / 'LICENSE-AGPL').write_text('unrelated text')
        assemble(self.options())
        self.assertEqual((self.output / 'LICENSE-AGPL').read_bytes(), original)

    def test_duplicate_platform_and_wrong_builder_are_rejected(self) -> None:
        original = copy.deepcopy(self.document)
        self.document['platforms'][1] = {
            **self.document['platforms'][0],
            'workflow_run': 'https://github.com/kubijo/garmin-toolkit/actions/runs/124',
        }
        with self.assertRaisesRegex(ValueError, 'Duplicate release platform'):
            assemble(self.options())
        self.document = original
        self.document['platforms'][0]['builder_system'] = 'aarch64-linux'
        with self.assertRaisesRegex(ValueError, 'Builder system'):
            assemble(self.options())

    def test_wrong_image_architecture_version_or_config_digest_is_rejected(self) -> None:
        original = copy.deepcopy(self.document)
        self.document['platforms'][0] = self.image('amd64', actual='arm64')
        with self.assertRaisesRegex(ValueError, 'linux/amd64'):
            assemble(self.options())
        self.document['platforms'][0] = self.image('amd64', version='0.2.0')
        with self.assertRaisesRegex(ValueError, 'labels differ'):
            assemble(self.options())
        self.document['platforms'][0] = self.image('amd64', tag='ghcr.io/kubijo/garmin-toolkit-hass:9.0.0')
        with self.assertRaisesRegex(ValueError, 'repository/version'):
            assemble(self.options())
        self.document = original
        self.document['platforms'][0] = self.image('amd64')
        self.document['platforms'][0]['config_digest'] = 'sha256:' + 'f' * 64
        with self.assertRaisesRegex(ValueError, 'config digest'):
            assemble(self.options())

    def test_payloads_must_be_complete_regular_nonempty_and_exclusive(self) -> None:
        target = self.artifacts / 'garmin-toolkit-hass-0.1.0-amd64.spdx.json'
        original = target.read_bytes()
        for condition in ('missing', 'empty', 'symlink'):
            target.unlink()
            if condition == 'empty':
                target.touch()
            elif condition == 'symlink':
                target.symlink_to(self.source / 'README.md')
            with self.subTest(condition=condition), self.assertRaises((ValueError, OSError)):
                assemble(self.options())
            if not target.exists() and not target.is_symlink():
                target.write_bytes(original)
            else:
                target.unlink()
                target.write_bytes(original)
        (self.artifacts / 'unexpected').mkdir()
        with self.assertRaisesRegex(ValueError, 'extra='):
            assemble(self.options())
        self.assertFalse(self.output.exists())

    def test_tampered_payload_is_rejected_even_with_rewritten_checksum_file(self) -> None:
        digest = assemble(self.options())
        payload = self.output / 'LICENSE-AGPL'
        original = payload.read_bytes()
        previous = hashlib.sha256(original).hexdigest()
        payload.write_bytes(b'X' + original[1:])
        current = hashlib.sha256(payload.read_bytes()).hexdigest()
        checksum = self.output / 'SHA256SUMS'
        checksum.write_text(checksum.read_text().replace(previous, current))
        with self.assertRaisesRegex(ValueError, 'content or role'):
            verify(self.verify_options(digest))

    def test_producing_receipt_binds_archive_and_sbom_bytes(self) -> None:
        for suffix in ('oci.tar', 'spdx.json'):
            path = self.artifacts / ('garmin-toolkit-hass-0.1.0-amd64.' + suffix)
            original = path.read_bytes()
            path.write_bytes(original + b'\n')
            with self.subTest(suffix=suffix), self.assertRaisesRegex(ValueError, 'hash differs from the receipt'):
                assemble(self.options())
            path.write_bytes(original)

    def test_image_reader_rejects_ambiguous_and_nonregular_metadata(self) -> None:
        for condition in ('duplicate', 'link', 'duplicate-json-key'):
            path = self.root / 'invalid.tar'
            with tarfile.open(path, 'w') as image:
                payload = b'{"imageLayoutVersion":"1.0.0"}'
                if condition == 'duplicate-json-key':
                    payload = b'{"imageLayoutVersion":"1.0.0","imageLayoutVersion":"other"}'
                member = tarfile.TarInfo('oci-layout')
                member.size = len(payload)
                if condition == 'link':
                    member.type = tarfile.SYMTYPE
                    member.linkname = 'elsewhere'
                image.addfile(member, io.BytesIO(payload) if condition != 'link' else None)
                if condition == 'duplicate':
                    image.addfile(member, io.BytesIO(payload))
            with (
                self.subTest(condition=condition),
                self.assertRaisesRegex(ValueError, 'metadata|regular|Duplicate JSON key'),
            ):
                inspect_archive(path)

    def test_manifest_hash_and_expected_source_identity_are_independent_inputs(self) -> None:
        digest = assemble(self.options())
        options = self.verify_options(digest)
        with self.assertRaisesRegex(ValueError, 'expected source'):
            verify(Verify(directory=self.output, manifest_sha256=digest, version='0.2.0', commit=options.commit))
        manifest = self.output / 'release.json'
        document = json.loads(manifest.read_text())
        document['source']['commit'] = 'f' * 40
        manifest.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'expected SHA-256'):
            verify(options)

    def test_schema_and_inventory_reject_forged_manifest_structure(self) -> None:
        assemble(self.options())
        path = self.output / 'release.json'
        original = json.loads(path.read_text())
        cases: list[tuple[str, Any]] = [
            ('status', 'published'),
            ('schema_version', 2),
            ('acceptance', [{'result': 'passed'}]),
            ('files', [original['files'][0]] * 9),
        ]
        for field, value in cases:
            document = copy.deepcopy(original)
            document[field] = value
            path.write_bytes(json_bytes(document))
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            with self.subTest(field=field), self.assertRaises(ValueError):
                verify(self.verify_options(digest))
        document = copy.deepcopy(original)
        document['files'][0]['name'] = '../outside'
        path.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'Invalid release'):
            verify(self.verify_options(hashlib.sha256(path.read_bytes()).hexdigest()))

    def test_checksum_inventory_and_strict_dirspec_cannot_omit_or_add_files(self) -> None:
        digest = assemble(self.options())
        checksum = self.output / 'SHA256SUMS'
        original = checksum.read_bytes()
        checksum.write_bytes(original.split(b'\n', 1)[1])
        with self.assertRaisesRegex(ValueError, 'SHA256SUMS'):
            verify(self.verify_options(digest))
        checksum.write_bytes(original)
        (self.output / 'undeclared').write_text('extra')
        checked = subprocess.run(
            ['dirspec', str(self.output), '--spec', str(CONTRACT / 'hass-release.dirspec.toml')],
            capture_output=True,
            check=False,
        )
        self.assertEqual(checked.returncode, 1)
        with self.assertRaisesRegex(ValueError, 'extra='):
            verify(self.verify_options(digest))

    def test_existing_output_and_failed_shape_check_leave_no_partial_output(self) -> None:
        options = self.options()
        with (
            patch('hass_release.check_shape', side_effect=ValueError('shape rejected')),
            self.assertRaisesRegex(ValueError, 'shape rejected'),
        ):
            assemble(options)
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.root.glob('.hass-release.*')), [])
        assemble(options)
        original = (self.output / 'release.json').read_bytes()
        with self.assertRaisesRegex(ValueError, 'already exists'):
            assemble(options)
        self.assertEqual((self.output / 'release.json').read_bytes(), original)

    def test_real_cli_verifies_and_reports_expected_failure_without_traceback(self) -> None:
        self.document['platforms'][0]['workflow_run'] = None
        self.document['platforms'][1]['workflow_run'] = None
        self.save_receipt()
        entrypoint = str(Path(__file__).with_name('hass_release.py'))
        # CI may force colors; select plain output independently of the parent environment.
        environment = {**os.environ, 'NO_COLOR': '1', 'FORCE_COLOR': '0', 'CLICOLOR_FORCE': '0'}
        assembled = subprocess.run(
            [
                sys.executable,
                entrypoint,
                'assemble',
                '--source',
                str(self.source),
                '--receipt',
                str(self.receipt),
                '--artifacts',
                str(self.artifacts),
                '--output',
                str(self.output),
            ],
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )
        self.assertEqual(assembled.returncode, 0, assembled.stdout + assembled.stderr)
        self.assertIn('Candidate assembled', assembled.stdout)
        digest = hashlib.sha256((self.output / 'release.json').read_bytes()).hexdigest()
        command = [
            sys.executable,
            str(Path(__file__).with_name('hass_release.py')),
            'verify',
            '--directory',
            str(self.output),
            '--manifest-sha256',
            digest,
            '--version',
            '0.1.0',
            '--commit',
            self.document['source']['commit'],
        ]
        verified = subprocess.run(command, capture_output=True, text=True, check=False, env=environment)
        self.assertEqual(verified.returncode, 0, verified.stdout + verified.stderr)
        self.assertIn('Local candidate content verified', verified.stdout)
        (self.output / 'LICENSE-AGPL').unlink()
        rejected = subprocess.run(command, capture_output=True, text=True, check=False, env=environment)
        self.assertEqual(rejected.returncode, 1, rejected.stdout + rejected.stderr)
        self.assertIn('Release check failed', rejected.stderr)
        self.assertNotIn('Traceback', rejected.stderr)
        self.assertNotIn('\x1b', rejected.stdout + rejected.stderr)


if __name__ == '__main__':
    unittest.main()
