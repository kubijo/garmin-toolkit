"""Two-platform publication must retain every native input and independently qualify both guests."""

import copy
import errno
import os
import shutil
import tarfile
import unittest
from pathlib import Path
from typing import cast
from unittest.mock import patch

import yaml

import hass_ci
import hass_preflight
import hass_publish as publication
import test_hass_release as release_fixtures
import test_hass_repository as repository_fixtures
from hass_release import assemble
from hass_release_contract import Release, copy_payload_file
from hass_repository import generate_repository
from json_data import Object, json_bytes, object_value, read_json


class MultiplatformTests(unittest.TestCase):
    def setUp(self) -> None:
        fixture = release_fixtures.ReleaseTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture = fixture
        fixture.split_sources('aarch64')
        assemble(fixture.options())
        self.document = cast(Release, read_json(fixture.output / 'release.json'))
        self.directory = fixture.root / 'publication'
        self.directory.mkdir()
        shutil.copytree(fixture.output, self.directory / 'candidate')
        self.builds, self.sources = fixture.root / 'native', fixture.root / 'sources'
        self.builds.mkdir()
        self.sources.mkdir()
        self.evidence = fixture.root / 'evidence'
        self.staging: Object = {
            'repository_url': publication.REMOTE + '#hass-candidate-123-1',
            'branch': 'hass-candidate-123-1',
            'commit': 'a' * 40,
            'tag': '0.1.0-candidate.123.1',
            'previous': '',
            'index_digest': self.document['image']['index_digest'],
        }
        (self.directory / 'staging.json').write_bytes(json_bytes(self.staging))
        for platform in self.document['platforms']:
            architecture = platform['architecture']
            native, sources, evidence = (
                self.builds / architecture,
                self.sources / architecture,
                self.evidence / architecture,
            )
            for path in (native, sources, evidence):
                path.mkdir(parents=True)
            receipt = {
                'schema_version': 1,
                'version': self.document['version'],
                'source': self.document['source'],
                'image_repository': publication.IMAGE,
                'platform': platform,
            }
            for root in (native, evidence):
                (root / 'native-build.json').write_bytes(json_bytes(receipt))
            for path in fixture.artifacts.glob('*-' + architecture + '*'):
                if '-sources.' in path.name:
                    shutil.copyfile(path, sources / ('dependency-' + path.name.split('-' + architecture + '-', 1)[1]))
                else:
                    shutil.copyfile(path, native / path.name)
            installed = {
                'architecture': 'arm64' if architecture == 'aarch64' else architecture,
                'archive_version': self.document['version'],
                'version': self.staging['tag'],
                'registry_digest': self.document['image']['index_digest'],
                'repository_url': self.staging['repository_url'],
                **{key: platform[key] for key in ('manifest_digest', 'config_digest', 'archive_sha256')},
            }
            workflow = {
                'passed': True,
                'restart': True,
                'cold_restore': True,
                'post_backup_mutation_reverted': True,
                'haos_reboot': {'before': 'a', 'after': 'b'},
                'lifecycle': {
                    **dict.fromkeys(
                        ('upgrade', 'recovery', 'app_data_removed', 'reinstall_empty', 'backup_retained'), True
                    ),
                    'baseline': 'metadata-only fixture; candidate filesystem; no previous public release',
                    'migration_compatibility': False,
                },
            }
            for name, document in (
                ('installation.json', installed),
                ('lifecycle-installation.json', {**installed, 'version': self.document['version']}),
                ('workflow.json', workflow),
                ('lifecycle-workflow.json', workflow),
            ):
                (evidence / name).write_bytes(json_bytes(document))
            for name in publication.EVIDENCE_FILES:
                if not (evidence / name).exists():
                    (evidence / name).write_bytes(b'Synthetic acceptance capture\n')
        repository = repository_fixtures.RepositoryTests()
        repository.setUp()
        self.addCleanup(repository.doCleanups)
        generate_repository(repository.options(('amd64', 'aarch64')))
        self.repository = repository.output
        self.registry: Object = {
            **{key: self.document[key] for key in ('schema_version', 'version', 'source', 'platforms', 'image')},
            'acceptance': [],
            'registry_reference': None,
        }
        self.addCleanup(patch.stopall)
        patch.dict(
            os.environ,
            {
                'GITHUB_REPOSITORY': publication.REPOSITORY,
                'GITHUB_REF': 'refs/tags/v0.1.0',
                'GITHUB_SHA': self.document['source']['commit'],
            },
        ).start()

    def test_preparation_collects_and_rechecks_sources_for_both_architectures(self) -> None:
        output = self.fixture.root / 'prepared'
        with patch.object(publication, 'prepare_registry', return_value=self.registry) as prepare:
            publication.prepare_publication(
                publication.PreparePublication(
                    source=self.fixture.source,
                    build=self.builds,
                    sources=self.sources,
                    repository=self.repository,
                    output=output,
                )
            )
        self.assertEqual(
            set(prepare.call_args.args[0].builds), {self.builds / name for name in publication.ARCHITECTURES}
        )
        document = publication.checked_candidate(self.fixture.source, output)
        self.assertEqual(document.get('source_parts'), {'aarch64': 2, 'amd64': 1})
        config = yaml.safe_load((output / 'repository/garmin_toolkit/config.yaml').read_text())
        self.assertEqual(config['arch'], ['aarch64', 'amd64'])
        source_part = next((output / 'candidate').glob('*-aarch64-sources.tar.gz.002'))
        self.assertTrue(source_part.samefile(self.sources / 'aarch64/dependency-sources.tar.gz.002'))
        source_part.write_bytes(b'corrupt ARM64 source part')
        with self.assertRaises(ValueError):
            publication.checked_candidate(self.fixture.source, output)

    def test_missing_native_platform_is_rejected_before_staging(self) -> None:
        shutil.rmtree(self.builds / 'aarch64')
        with self.assertRaisesRegex(ValueError, 'both native'):
            publication.build_directories(self.builds)

    def test_cross_filesystem_payloads_copy_and_linked_inputs_are_rejected(self) -> None:
        source = self.sources / 'aarch64/dependency-sources.tar.gz'
        destination = self.fixture.root / 'copied-source.tar.gz'
        with patch('hass_release_contract.os.link', side_effect=OSError(errno.EXDEV, 'Different filesystem')):
            copy_payload_file(source, destination)
        self.assertEqual(destination.read_bytes(), source.read_bytes())
        self.assertFalse(destination.samefile(source))
        linked = self.fixture.root / 'linked-source.tar.gz'
        linked.symlink_to(source)
        with self.assertRaises(ValueError):
            copy_payload_file(linked, self.fixture.root / 'rejected-source.tar.gz')

    def test_both_guest_reports_are_required_and_cannot_be_crossed(self) -> None:
        publication.verify_acceptance(self.evidence, self.document, self.staging)
        target = self.evidence / 'aarch64/installation.json'
        original = target.read_bytes()
        shutil.copyfile(self.evidence / 'amd64/installation.json', target)
        with self.assertRaisesRegex(ValueError, 'acceptance'):
            publication.verify_acceptance(self.evidence, self.document, self.staging)
        target.write_bytes(original)
        workflow = self.evidence / 'aarch64/lifecycle-workflow.json'
        document = object_value(read_json(workflow))
        document['lifecycle'] = {'upgrade': True}
        workflow.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'acceptance'):
            publication.verify_acceptance(self.evidence, self.document, self.staging)

    def test_first_arm64_release_cannot_claim_unqualified_migration_coverage(self) -> None:
        path = self.evidence / 'aarch64/lifecycle-workflow.json'
        original = object_value(read_json(path))
        for key in ('baseline', 'migration_compatibility'):
            document = copy.deepcopy(original)
            object_value(document['lifecycle']).pop(key)
            path.write_bytes(json_bytes(document))
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'fixture baseline'):
                publication.verify_acceptance(self.evidence, self.document, self.staging)
        document = copy.deepcopy(original)
        object_value(document['lifecycle'])['migration_compatibility'] = True
        path.write_bytes(json_bytes(document))
        with self.assertRaisesRegex(ValueError, 'fixture baseline'):
            publication.verify_acceptance(self.evidence, self.document, self.staging)

    def test_new_releases_require_matching_published_baseline_evidence_on_both_architectures(self) -> None:
        self.document['version'] = '0.1.4'
        self.document['source']['tag'] = 'v0.1.4'
        self.staging['tag'] = '0.1.4-candidate.123.1'
        for platform in self.document['platforms']:
            architecture = platform['architecture']
            evidence = self.evidence / architecture
            native = object_value(read_json(evidence / 'native-build.json'))
            native.update(version='0.1.4', source=self.document['source'])
            (evidence / 'native-build.json').write_bytes(json_bytes(native))
            for name in ('installation.json', 'lifecycle-installation.json'):
                image = object_value(read_json(evidence / name))
                image.update(
                    archive_version='0.1.4',
                    version=self.staging['tag'] if name == 'installation.json' else '0.1.4',
                )
                (evidence / name).write_bytes(json_bytes(image))
            path = evidence / 'lifecycle-workflow.json'
            workflow = object_value(read_json(path))
            object_value(workflow['lifecycle']).update(
                migration_compatibility=True,
                baseline='published release v0.1.3',
                candidate_version='0.1.4',
                candidate_archive_version='0.1.4',
                published_baseline={
                    'architecture': architecture,
                    'version': '0.1.3',
                    'release': 'v0.1.3',
                    'source_commit': '1' * 40,
                    'archive_sha256': '2' * 64,
                    'manifest_digest': 'sha256:' + '3' * 64,
                    'config_digest': 'sha256:' + '4' * 64,
                    'registry_index_digest': 'sha256:' + '5' * 64,
                    'release_manifest_sha256': '6' * 64,
                    'signatures_verified': True,
                },
            )
            path.write_bytes(json_bytes(workflow))
        publication.verify_acceptance(self.evidence, self.document, self.staging)
        for architecture in publication.ARCHITECTURES:
            path = self.evidence / architecture / 'lifecycle-workflow.json'
            original = path.read_bytes()
            workflow = object_value(read_json(path))
            baseline = object_value(object_value(workflow['lifecycle'])['published_baseline'])
            baseline['architecture'] = 'amd64' if architecture == 'aarch64' else 'aarch64'
            path.write_bytes(json_bytes(workflow))
            with self.subTest(crossed=architecture), self.assertRaisesRegex(ValueError, 'authenticated older release'):
                publication.verify_acceptance(self.evidence, self.document, self.staging)
            object_value(workflow['lifecycle']).pop('published_baseline')
            path.write_bytes(json_bytes(workflow))
            with self.subTest(missing=architecture), self.assertRaises(ValueError):
                publication.verify_acceptance(self.evidence, self.document, self.staging)
            path.write_bytes(original)

    def test_final_installation_of_each_architecture_must_pass_before_qualification(self) -> None:
        for failed in ('aarch64', 'amd64', None):
            output = self.fixture.root / ('final-' + str(failed))

            def promoted(_options: object, selected: Path = output) -> None:
                selected.mkdir()
                (selected / 'staging.json').write_bytes(json_bytes(self.staging))
                for architecture in publication.ARCHITECTURES:
                    (selected / 'native' / architecture).mkdir(parents=True)

            def acceptance(
                directory: Path, evidence: Path, *arguments: str, rejected: str | None = failed, **_kwargs: object
            ) -> None:
                self.assertEqual(directory.name, evidence.name)
                self.assertEqual(arguments[arguments.index('--arch') + 1], directory.name)
                if directory.name == rejected:
                    raise RuntimeError('Guest failed: ' + directory.name)

            with (
                self.subTest(failed=failed),
                patch.object(hass_ci, 'PUBLICATION', output),
                patch.object(hass_ci, 'release_guard'),
                patch.object(hass_ci, 'login'),
                patch.object(hass_ci, 'promote_draft', side_effect=promoted),
                patch.object(hass_ci, 'acceptance', side_effect=acceptance) as check,
                patch.object(hass_ci, 'qualify_promotion') as qualify,
                patch.object(hass_ci, 'rollback_store') as rollback,
            ):
                if failed is None:
                    hass_ci.promote()
                    qualify.assert_called_once()
                    rollback.assert_not_called()
                    self.assertEqual(check.call_count, 2)
                else:
                    with self.assertRaisesRegex(RuntimeError, 'Guest failed'):
                        hass_ci.promote()
                    qualify.assert_not_called()
                    rollback.assert_called_once()
                    self.assertEqual(check.call_count, 1 if failed == 'aarch64' else 2)

    def test_signed_archive_requires_complete_architecture_separated_evidence(self) -> None:
        publication.archive_evidence(self.directory, self.evidence, self.document['platforms'])
        archive = self.directory / 'haos-acceptance.tar.gz'
        restored = self.fixture.root / 'restored'
        staging = publication.extract_evidence(archive, restored, self.document['platforms'])
        publication.verify_acceptance(restored, self.document, staging)
        for mode in ('missing-arm64', 'mixed-layout', 'duplicate', 'traversal'):
            damaged = self.fixture.root / (mode + '.tar.gz')
            with tarfile.open(archive, 'r:gz') as original, tarfile.open(damaged, 'w:gz') as result:
                for member in original:
                    if mode == 'missing-arm64' and member.name == 'aarch64/workflow.json':
                        continue
                    result.addfile(member, original.extractfile(member))
                if mode != 'missing-arm64':
                    name = {
                        'mixed-layout': 'workflow.json',
                        'duplicate': 'staging.json',
                        'traversal': '../workflow.json',
                    }[mode]
                    result.add(self.evidence / 'amd64/workflow.json', arcname=name)
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                publication.extract_evidence(damaged, self.fixture.root / mode, self.document['platforms'])
        with self.assertRaisesRegex(ValueError, 'Unexpected'):
            publication.extract_evidence(archive, self.fixture.root / 'legacy-layout')

    def test_promotion_rehydrates_each_signed_platform_without_rebuilding(self) -> None:
        publication.archive_evidence(self.directory, self.evidence, self.document['platforms'])
        draft = self.fixture.root / 'draft'
        shutil.copytree(self.directory / 'candidate', draft)
        shutil.copyfile(self.directory / 'haos-acceptance.tar.gz', draft / 'haos-acceptance.tar.gz')
        digests = {
            self.document['image']['index_digest'],
            *(row['manifest_digest'] for row in self.document['platforms']),
        }
        for name in ('artifacts', 'checksums', 'acceptance', *(digest.replace(':', '-') for digest in digests)):
            (draft / (name + '.sigstore.jsonl')).write_text('Synthetic bundle\n')

        def command(*args: str, cwd: Path | None = None) -> str:
            del cwd
            if args[:3] == ('gh', 'release', 'view'):
                return '{"isDraft": true}'
            if args[:3] == ('gh', 'release', 'download'):
                destination = Path(args[args.index('--dir') + 1])
                for path in draft.iterdir():
                    shutil.copyfile(path, destination / path.name)
                return ''
            if args[:2] == ('nix', 'build'):
                self.assertTrue(args[2].endswith('#garmin-hass-app-repository'))
                return str(self.repository)
            raise AssertionError('Unexpected publication mutation: ' + str(args))

        output = self.fixture.root / 'promoted'
        with (
            patch.object(publication, 'command', side_effect=command),
            patch('hass_release.verify_provenance'),
            patch.object(publication, 'attest'),
            patch.object(publication, 'promote_candidate') as promote,
        ):
            publication.promote_draft(publication.PromoteDraft(source=self.fixture.source, output=output))
        promote.assert_called_once()
        for platform in self.document['platforms']:
            native = output / 'native' / platform['architecture']
            self.assertEqual(object_value(read_json(native / 'native-build.json'))['platform'], platform)
            for path in native.glob('*.oci.tar'):
                self.assertTrue(path.samefile(output / 'candidate' / path.name))
        self.assertEqual(
            yaml.safe_load((output / 'repository/garmin_toolkit/config.yaml').read_text())['arch'], ['aarch64', 'amd64']
        )

    def test_branch_verification_binds_both_manifest_signatures_and_guest_reports(self) -> None:
        registry = copy.deepcopy(self.registry)
        object_value(registry['source'])['tag'] = None
        identity = object_value(registry['source'])
        for architecture in publication.ARCHITECTURES:
            path = self.evidence / architecture / 'native-build.json'
            native = object_value(read_json(path))
            native['source'] = identity
            path.write_bytes(json_bytes(native))
        output = self.fixture.root / 'preflight'
        reference = 'refs/heads/dev'
        with (
            patch.object(hass_preflight, 'guard', return_value=('0.1.0', identity)),
            patch.object(hass_preflight, 'prepare_registry', return_value=registry),
            patch.dict(os.environ, GITHUB_REF=reference),
        ):
            hass_preflight.prepare(
                publication.PreparePublication(
                    source=self.fixture.source,
                    build=self.builds,
                    sources=self.sources,
                    repository=self.repository,
                    output=output,
                )
            )
            (output / 'staging.json').write_bytes(json_bytes(self.staging))
            hass_preflight.package(
                publication.ReleaseEvidence(
                    source=self.fixture.source, directory=output, build=self.builds, evidence=self.evidence
                )
            )
            bundles = output / 'bundles'
            bundles.mkdir()
            digests = {
                self.document['image']['index_digest'],
                *(row['manifest_digest'] for row in self.document['platforms']),
            }
            for name in ('artifacts', 'checksums', 'acceptance', *(digest.replace(':', '-') for digest in digests)):
                (bundles / (name + '.sigstore.jsonl')).write_text('Synthetic bundle\n')
            with patch.object(hass_preflight, 'attest_reference') as attest:
                hass_preflight.verify(self.fixture.source, output, bundles)
            subjects = {call.args[0] for call in attest.call_args_list}
            self.assertTrue({'oci://' + publication.IMAGE + '@' + digest for digest in digests}.issubset(subjects))
            arm = next(row for row in self.document['platforms'] if row['architecture'] == 'aarch64')
            (bundles / (arm['manifest_digest'].replace(':', '-') + '.sigstore.jsonl')).unlink()
            with patch.object(hass_preflight, 'attest_reference'), self.assertRaises(ValueError):
                hass_preflight.verify(self.fixture.source, output, bundles)
