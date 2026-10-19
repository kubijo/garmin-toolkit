"""Branch artifacts share release packaging but cannot qualify as tagged releases."""

import os
import shutil
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

import hass_preflight as preflight
import hass_publish
import test_hass_publish as publication_fixtures
import test_hass_repository as repository_fixtures
from hass_attestations import WORKFLOW, attest_reference
from hass_publish import (
    EVIDENCE_FILES,
    PreparePublication,
    ReleaseGuard,
    StagePublication,
    archive_evidence,
    release_guard,
)
from hass_registry import PrepareRegistry
from hass_release_contract import inventory
from json_data import Object, json_bytes, object_value, read_json


class PreflightTests(unittest.TestCase):
    def setUp(self) -> None:
        fixture = publication_fixtures.PublicationTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture = fixture
        self.source = fixture.fixture.source
        fixture.fixture.git('tag', '-d', 'v0.1.0')
        fixture.fixture.git('checkout', '-b', 'dev')
        self.commit = fixture.document['source']['commit']
        self.reference = 'refs/heads/dev'
        patch.dict(
            os.environ,
            {
                'GITHUB_ACTIONS': 'true',
                'GITHUB_EVENT_NAME': 'workflow_dispatch',
                'GITHUB_REF': self.reference,
                'GITHUB_SHA': self.commit,
                'GITHUB_WORKFLOW_REF': WORKFLOW + '@' + self.reference,
            },
        ).start()
        self.directory = fixture.fixture.root / 'preflight'
        self.candidate = self.directory / 'candidate'
        self.candidate.mkdir(parents=True)
        for path in fixture.fixture.output.iterdir():
            if path.name not in ('release.json', 'SHA256SUMS'):
                shutil.copyfile(path, self.candidate / path.name)
        self.registry: Object = {
            **{key: fixture.document[key] for key in ('schema_version', 'version', 'source', 'image', 'platforms')},
            'source': {**fixture.document['source'], 'tag': None},
            'registry_reference': None,
            'acceptance': [],
        }
        (self.candidate / 'registry.json').write_bytes(json_bytes(self.registry))
        self.inputs = preflight.payload(self.registry, next(self.candidate.glob('*-amd64-sources.json')))
        (self.candidate / 'SHA256SUMS').write_bytes(preflight.checksum_bytes(self.candidate, self.inputs))
        fixture.save_evidence()
        native = object_value(read_json(fixture.evidence / 'native-build.json'))
        native['source'] = self.registry['source']
        (fixture.evidence / 'native-build.json').write_bytes(json_bytes(native))
        for name in EVIDENCE_FILES:
            path = fixture.evidence / name
            if not path.exists():
                path.write_bytes(b'Synthetic acceptance fixture\n')
        (self.directory / 'staging.json').write_bytes(
            json_bytes(
                {
                    **fixture.staging,
                    'branch': 'hass-candidate-1-1',
                    'commit': 'a' * 40,
                    'index_digest': self.inputs['image']['index_digest'],
                }
            )
        )
        archive_evidence(self.directory, fixture.evidence)
        self.bundles = fixture.bundles
        for name in (
            'artifacts.sigstore.jsonl',
            'checksums.sigstore.jsonl',
            'acceptance.sigstore.jsonl',
            self.inputs['image']['index_digest'].replace(':', '-') + '.sigstore.jsonl',
            self.inputs['platforms'][0]['manifest_digest'].replace(':', '-') + '.sigstore.jsonl',
        ):
            (self.bundles / name).write_text('Synthetic signature fixture\n')

    def test_branch_guards_reject_other_events_repositories_references_and_commits(self) -> None:
        self.assertEqual(preflight.guard(self.source)[1]['commit'], self.commit)
        for name, value in (
            ('GITHUB_ACTIONS', 'false'),
            ('GITHUB_EVENT_NAME', 'pull_request'),
            ('GITHUB_REPOSITORY', 'other/fork'),
            ('GITHUB_REF', 'refs/tags/v0.1.0'),
            ('GITHUB_SHA', 'f' * 40),
            ('GITHUB_WORKFLOW_REF', WORKFLOW + '@refs/heads/main'),
        ):
            with self.subTest(name=name), patch.dict(os.environ, {name: value}), self.assertRaises(ValueError):
                preflight.guard(self.source)
        with self.assertRaises(ValueError):
            release_guard(ReleaseGuard(source=self.source))

    def test_preparation_reuses_release_inventory_and_committed_notices_without_a_tag(self) -> None:
        sources = self.fixture.fixture.root / 'preflight-sources'
        sources.mkdir()
        for path in self.candidate.glob('*-amd64-sources*'):
            shutil.copyfile(path, sources / ('dependency-' + path.name.split('-amd64-', 1)[1]))
        repository = repository_fixtures.RepositoryTests()
        repository.setUp()
        self.addCleanup(repository.doCleanups)
        repository_fixtures.generate_repository(repository.options())
        output = self.fixture.fixture.root / 'prepared-preflight'

        def registry(options: PrepareRegistry) -> Object:
            options.output.mkdir()
            return self.registry

        with patch.object(preflight, 'prepare_registry', side_effect=registry):
            preflight.prepare(
                PreparePublication(
                    source=self.source,
                    build=self.fixture.fixture.artifacts,
                    sources=sources,
                    repository=repository.output,
                    output=output,
                )
            )
        document, inputs = preflight.check_candidate(self.source, output / 'candidate')
        self.assertIsNone(object_value(document['source'])['tag'])
        self.assertEqual(
            {spec['name'] for spec in inventory(inputs)}, {spec['name'] for spec in inventory(self.inputs)}
        )
        self.assertEqual((output / 'candidate/COPYRIGHT.md').read_bytes(), (self.source / 'COPYRIGHT.md').read_bytes())
        self.assertFalse((output / 'candidate/release.json').exists())

    def test_independent_downloads_verify_every_file_and_immutable_image_subject(self) -> None:
        download = self.fixture.fixture.root / 'downloaded'
        shutil.copytree(self.directory, download)
        downloaded_bundles = download / 'bundles'
        shutil.copytree(self.bundles, downloaded_bundles)
        with patch.object(preflight, 'attest_reference') as attest:
            preflight.verify(self.source, download, downloaded_bundles)
        calls = attest.call_args_list
        self.assertEqual(Path(calls[0].args[0]).name, 'SHA256SUMS')
        subjects = {call.args[0] for call in calls}
        self.assertIn('oci://' + preflight.IMAGE + '@' + self.inputs['image']['index_digest'], subjects)
        self.assertIn(str(download / 'candidate/registry.json'), subjects)
        self.assertTrue(all(call.args[2:] == (self.reference, self.commit) for call in calls))
        report = object_value(read_json(download / 'preflight-verification.json'))
        self.assertIs(report['passed'], True)
        self.assertEqual(report['source_commit'], self.commit)
        self.assertEqual(report['reference'], self.reference)

    def test_failed_checksum_signature_stops_before_reading_candidate_metadata(self) -> None:
        with (
            patch.object(preflight, 'attest_reference', side_effect=ValueError('untrusted signature')),
            patch.object(preflight, 'check_candidate') as check,
            self.assertRaisesRegex(ValueError, 'untrusted signature'),
        ):
            preflight.verify(self.source, self.directory, self.bundles)
        check.assert_not_called()

    def test_payload_cannot_be_rebound_by_rewriting_checksums_after_authentication(self) -> None:
        def changed(_subject: str, _bundle: Path, _reference: str, _commit: str) -> None:
            (self.candidate / 'COPYRIGHT.md').write_text('Substituted payload\n')
            (self.candidate / 'SHA256SUMS').write_bytes(preflight.checksum_bytes(self.candidate, self.inputs))

        with (
            patch.object(preflight, 'attest_reference', side_effect=changed),
            self.assertRaisesRegex(ValueError, 'checksums'),
        ):
            preflight.verify(self.source, self.directory, self.bundles)

    def test_altered_missing_extra_and_linked_payloads_are_rejected(self) -> None:
        for mode in ('altered', 'missing', 'extra', 'symlink', 'wrong-receipt'):
            with self.subTest(mode=mode):
                directory = self.fixture.fixture.root / mode
                shutil.copytree(self.directory, directory)
                candidate = directory / 'candidate'
                target = candidate / 'COPYRIGHT.md'
                if mode == 'altered':
                    target.write_text('Altered copyright\n')
                elif mode == 'missing':
                    (candidate / 'SHA256SUMS').unlink()
                elif mode == 'extra':
                    (candidate / 'unexpected').write_text('Extra file\n')
                elif mode == 'symlink':
                    target.unlink()
                    target.symlink_to(self.candidate / 'COPYRIGHT.md')
                else:
                    document = object_value(read_json(candidate / 'registry.json'))
                    object_value(document['source'])['commit'] = 'f' * 40
                    (candidate / 'registry.json').write_bytes(json_bytes(document))
                with patch.object(preflight, 'attest_reference'), self.assertRaises((ValueError, FileNotFoundError)):
                    preflight.verify(self.source, directory, self.bundles)

    def test_branch_certificate_policy_pins_workflow_ref_and_both_source_digests(self) -> None:
        with patch('hass_attestations.subprocess.run', return_value=subprocess.CompletedProcess([], 0)) as run:
            attest_reference(
                'synthetic-subject', self.bundles / 'checksums.sigstore.jsonl', self.reference, self.commit
            )
        command = run.call_args.args[0]
        self.assertNotIn('--signer-workflow', command)
        for name, expected in (
            ('--source-ref', self.reference),
            ('--source-digest', self.commit),
            ('--signer-digest', self.commit),
            ('--cert-identity', 'https://github.com/' + WORKFLOW + '@' + self.reference),
        ):
            self.assertEqual(command[command.index(name) + 1], expected)
        self.assertIn('--deny-self-hosted-runners', command)

    def test_branch_staging_does_not_reconcile_or_modify_the_public_store(self) -> None:
        digest = self.inputs['image']['index_digest']
        with (
            patch.dict(os.environ, GITHUB_RUN_ID='1', GITHUB_RUN_ATTEMPT='1'),
            patch.object(hass_publish, 'store', side_effect=AssertionError('Public store accessed')),
            patch.object(hass_publish, 'branch_head', return_value='') as head,
            patch.object(hass_publish, 'command') as command,
            patch.object(hass_publish, 'verify_registry'),
            patch.object(hass_publish, 'image_digest', return_value=digest),
            patch.object(hass_publish, 'push_metadata', return_value='a' * 40) as push,
        ):
            hass_publish.stage_image(
                StagePublication(source=self.source, directory=self.directory, build=self.fixture.fixture.artifacts),
                '0.1.0',
                digest,
                '',
            )
        head.assert_called_once_with('hass-candidate-1-1')
        self.assertEqual(push.call_args.args[1:], ('hass-candidate-1-1', ''))
        self.assertEqual(command.call_args.args[-1], 'docker://' + preflight.IMAGE + ':0.1.0-candidate.1.1')
        self.assertFalse((self.directory / 'promotion.json').exists())

    def test_signed_archive_must_contain_successful_lifecycle_evidence_for_this_image(self) -> None:
        workflow = self.fixture.evidence / 'workflow.json'
        document = object_value(read_json(workflow))
        document['cold_restore'] = False
        workflow.write_bytes(json_bytes(document))
        (self.directory / 'haos-acceptance.tar.gz').unlink()
        archive_evidence(self.directory, self.fixture.evidence)
        with patch.object(preflight, 'attest_reference'), self.assertRaisesRegex(ValueError, 'acceptance'):
            preflight.verify(self.source, self.directory, self.bundles)

    def test_live_negative_checks_fail_if_invalid_provenance_is_accepted(self) -> None:
        with patch.object(preflight, 'attest_reference'), self.assertRaisesRegex(ValueError, 'accepted a deliberately'):
            preflight.qualify_rejections(self.source, self.directory, self.bundles)
