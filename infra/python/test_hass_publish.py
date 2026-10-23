"""Adversarial release gates and real disposable Git store promotion/rollback."""

import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import unittest
from collections.abc import Callable
from contextlib import chdir
from itertools import product
from pathlib import Path
from typing import cast
from unittest.mock import patch

import yaml

import hass_publish
import test_hass_release as release_fixtures
import test_hass_repository as repository_fixtures
from hass_attestations import VerifyProvenance
from hass_publish import (
    EVIDENCE_FILES,
    CleanupStaging,
    CompletePublication,
    PromoteDraft,
    ReleaseGuard,
    RollbackStore,
    StagePublication,
    assert_remote_tag,
    branch_head,
    cleanup_staging,
    complete_publication,
    promote_candidate,
    promote_draft,
    push_metadata,
    qualify_rejections,
    release_guard,
    rollback_store,
    stage_publication,
    verify_acceptance,
)
from hass_release import Verify, assemble, verify
from hass_release_contract import Release
from hass_repository import generate_repository
from json_data import Object, file_sha256, json_bytes


class PublicationTests(unittest.TestCase):
    def setUp(self) -> None:
        fixture = release_fixtures.ReleaseTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture = fixture
        fixture.document['platforms'] = fixture.document['platforms'][:1]
        for path in fixture.artifacts.glob('*-aarch64*'):
            path.unlink()
        fixture.save_receipt()
        assemble(fixture.options())
        self.document = cast(Release, json.loads((fixture.output / 'release.json').read_bytes()))
        self.directory = fixture.root / 'publication'
        self.directory.mkdir()
        shutil.copytree(fixture.output, self.directory / 'candidate')
        self.bundles, self.evidence = fixture.root / 'bundles', fixture.root / 'evidence'
        self.bundles.mkdir()
        self.evidence.mkdir()
        (self.bundles / 'checksums.sigstore.jsonl').write_text('Synthetic bundle\n')
        self.staging: Object = {
            'repository_url': 'https://github.com/kubijo/garmin-toolkit#hass-candidate-1-1',
            'previous': '',
        }
        (self.directory / 'staging.json').write_bytes(json_bytes(self.staging))
        platform = self.document['platforms'][0]
        self.installed: Object = {
            'registry_digest': self.document['image']['index_digest'],
            'manifest_digest': platform['manifest_digest'],
            'config_digest': platform['config_digest'],
            'archive_sha256': platform['archive_sha256'],
            'repository_url': self.staging['repository_url'],
        }
        self.workflow: Object = {
            'passed': True,
            'restart': True,
            'cold_restore': True,
            'post_backup_mutation_reverted': True,
            'haos_reboot': {'before': 'a', 'after': 'b'},
        }
        self.save_evidence()
        self.addCleanup(patch.stopall)
        patch('hass_publish.assert_remote_tag').start()
        patch.dict(
            os.environ,
            {
                'GITHUB_REPOSITORY': 'kubijo/garmin-toolkit',
                'GITHUB_REF': 'refs/tags/v0.1.0',
                'GITHUB_SHA': fixture.document['source']['commit'],
            },
        ).start()

    def test_preparation_copies_and_qualifies_every_source_part(self) -> None:
        second = self.fixture.split_sources()
        sources = self.fixture.root / 'source-parts'
        sources.mkdir()
        for path in self.fixture.artifacts.glob('*-amd64-sources*'):
            shutil.copyfile(path, sources / ('dependency-' + path.name.split('-amd64-', 1)[1]))
        repository = repository_fixtures.RepositoryTests()
        repository.setUp()
        self.addCleanup(repository.doCleanups)
        generate_repository(repository.options())
        prepared = self.fixture.root / 'prepared'
        with patch('hass_publish.prepare_registry', return_value=self.fixture.document):
            hass_publish.prepare_publication(
                hass_publish.PreparePublication(
                    source=self.fixture.source,
                    build=self.fixture.artifacts,
                    sources=sources,
                    repository=repository.output,
                    output=prepared,
                )
            )
        candidate = hass_publish.checked_candidate(self.fixture.source, prepared)
        self.assertEqual(candidate.get('source_parts'), {'amd64': 2})
        self.assertEqual((prepared / 'candidate' / second.name).read_bytes(), second.read_bytes())

    def save_evidence(self) -> None:
        (self.evidence / 'installation.json').write_bytes(json_bytes(self.installed))
        (self.evidence / 'workflow.json').write_bytes(json_bytes(self.workflow))
        (self.evidence / 'native-build.json').write_bytes(
            json_bytes(
                {
                    'source': self.document['source'],
                    'platform': self.document['platforms'][0],
                }
            )
        )

        (self.evidence / 'lifecycle-workflow.json').write_bytes(
            json_bytes(
                {
                    'passed': True,
                    'lifecycle': {
                        key: True
                        for key in ('upgrade', 'recovery', 'app_data_removed', 'reinstall_empty', 'backup_retained')
                    },
                }
            )
        )
        (self.evidence / 'lifecycle-installation.json').write_bytes(json_bytes(self.installed))

    def test_tag_workflow_and_source_commit_must_agree(self) -> None:
        options = ReleaseGuard(source=self.fixture.source)
        self.assertEqual(release_guard(options)[0], '0.1.0')
        for key, value in (
            ('GITHUB_REF', 'refs/heads/main'),
            ('GITHUB_SHA', 'f' * 40),
            ('GITHUB_REPOSITORY', 'other/fork'),
        ):
            with self.subTest(key=key), patch.dict(os.environ, {key: value}), self.assertRaises(ValueError):
                release_guard(options)

    def test_lightweight_and_annotated_remote_tags_must_still_point_to_the_signed_commit(self) -> None:
        commit = self.document['source']['commit']
        reference = 'refs/tags/v0.1.0'
        for response in (
            commit + '\t' + reference,
            'a' * 40 + '\t' + reference + '\n' + commit + '\t' + reference + '^{}',
        ):
            with self.subTest(response=response), patch('hass_publish.git', return_value=response):
                assert_remote_tag('0.1.0', commit)
                with self.assertRaisesRegex(ValueError, 'moved'):
                    assert_remote_tag('0.1.0', 'f' * 40)
        with patch('hass_publish.git', return_value=''), self.assertRaisesRegex(ValueError, 'disappeared'):
            assert_remote_tag('0.1.0', commit)

    def test_missing_restart_restore_reboot_or_wrong_image_cannot_qualify(self) -> None:
        verify_acceptance(self.evidence, self.document, self.staging)
        for key in ('restart', 'cold_restore', 'post_backup_mutation_reverted'):
            original = self.workflow[key]
            self.workflow[key] = False
            self.save_evidence()
            with self.assertRaises(ValueError):
                verify_acceptance(self.evidence, self.document, self.staging)
            self.workflow[key] = original
        for reboot in ({'before': 'a'}, {'after': 'b'}, {'before': 'a', 'after': 'a'}):
            self.workflow['haos_reboot'] = reboot
            self.save_evidence()
            with self.subTest(reboot=reboot), self.assertRaises(ValueError):
                verify_acceptance(self.evidence, self.document, self.staging)
        self.workflow['haos_reboot'] = {'before': 'a', 'after': 'b'}
        self.installed['registry_digest'] = 'sha256:' + 'f' * 64
        self.save_evidence()
        with self.assertRaises(ValueError):
            verify_acceptance(self.evidence, self.document, self.staging)

    def test_promotion_and_rollback_use_exact_leases_and_preserve_history(self) -> None:
        remote, repository = self.fixture.root / 'remote.git', self.directory / 'repository'
        subprocess.run(['git', 'init', '--bare', '-q', str(remote)], check=True)
        app = repository / 'garmin_toolkit'
        app.mkdir(parents=True)
        (app / 'config.yaml').write_text('version: 0.1.0\n')
        with patch('hass_publish.REMOTE', str(remote)):
            first = push_metadata(repository, 'hass-apps', '')
            (app / 'config.yaml').write_text('version: 0.2.0\n')
            second = push_metadata(
                repository,
                'hass-apps',
                first,
                before_push=lambda commit, current, checkout: hass_publish.store().prepare(
                    checkout, self.directory, current, commit
                ),
            )
            self.assertEqual(branch_head('hass-apps'), second)
            (app / 'config.yaml').write_text('version: 0.3.0\n')
            with self.assertRaises(subprocess.CalledProcessError):
                push_metadata(repository, 'hass-apps', first)
            rollback_store(RollbackStore(directory=self.directory))
            self.assertEqual(branch_head('hass-apps'), first)

    def test_existing_version_conflict_stops_before_creating_a_release(self) -> None:
        (self.directory / 'haos-acceptance.tar.gz').write_bytes(b'Synthetic evidence')
        with (
            patch('hass_publish.checked_candidate', return_value=self.document),
            patch('hass_release.verify_provenance'),
            patch('hass_publish.qualify_rejections'),
            patch('hass_publish.attest'),
            patch('hass_publish.image_digest', return_value='sha256:' + 'f' * 64),
            patch('hass_publish.command', return_value='{"Tags":["0.1.0"]}') as command,
        ):
            with self.assertRaisesRegex(ValueError, 'refusing to replace'):
                complete_publication(
                    CompletePublication(
                        source=self.fixture.source,
                        directory=self.directory,
                        build=self.fixture.artifacts,
                        bundles=self.bundles,
                        evidence=self.evidence,
                    )
                )
            self.assertEqual(command.call_count, 1)

    def test_draft_downloads_are_reverified_before_publishing_or_changing_store(self) -> None:
        (self.directory / 'haos-acceptance.tar.gz').write_bytes(b'Synthetic evidence')
        events: list[str] = []
        assets: list[Path] = []

        def verified(_options: object) -> None:
            events.append('verify signatures')

        def advanced(*_arguments: object, **kwargs: object) -> str:
            cast(Callable[[str, str, Path], object], kwargs['before_push'])('f' * 40, '', self.directory)
            events.append('advance store')
            return 'f' * 40

        def command(*args: str, cwd: Path | None = None) -> str:
            del cwd
            if args[:2] == ('skopeo', 'list-tags'):
                return '{"Tags":[]}'
            if args[:3] == ('gh', 'release', 'upload'):
                assets.extend(Path(path) for path in args[6:])
            if args[:3] == ('gh', 'release', 'download'):
                destination = Path(args[args.index('--dir') + 1])
                for path in assets:
                    shutil.copyfile(path, destination / path.name)
            events.append(' '.join(args[:3]))
            return ''

        with (
            patch('hass_publish.checked_candidate', return_value=self.document),
            patch('hass_release.verify_provenance', side_effect=verified),
            patch('hass_publish.qualify_rejections'),
            patch('hass_publish.attest'),
            patch('hass_publish.command', side_effect=command),
            patch('hass_publish.image_digest', return_value=self.document['image']['index_digest']),
            patch('hass_publish.branch_head', return_value=''),
            patch('hass_publish.push_metadata', side_effect=advanced),
            patch('hass_publish.store'),
        ):
            complete_publication(
                CompletePublication(
                    source=self.fixture.source,
                    directory=self.directory,
                    build=self.fixture.artifacts,
                    bundles=self.bundles,
                    evidence=self.evidence,
                )
            )
            self.assertNotIn('gh release edit', events)
            self.assertNotIn('advance store', events)
            promote_candidate(
                StagePublication(source=self.fixture.source, directory=self.directory, build=self.fixture.artifacts),
                self.document,
                self.staging,
            )
        self.assertLess(events.index('gh release download'), events.index('gh release edit'))
        self.assertLess(events.index('gh release edit'), events.index('advance store'))
        self.assertEqual(events.count('verify signatures'), 2)

    def test_live_rejection_qualification_preserves_the_original_candidate(self) -> None:
        candidate = self.directory / 'candidate'
        original = {path.name: file_sha256(path) for path in candidate.iterdir()}
        options = VerifyProvenance(
            directory=candidate, bundles=self.bundles, version='0.1.0', commit=self.document['source']['commit']
        )
        raw = b'Synthetic existing OCI index'
        qualified = cast(
            Release,
            {
                **self.document,
                'image': {**self.document['image'], 'index_digest': 'sha256:' + hashlib.sha256(raw).hexdigest()},
            },
        )
        missing: set[str] = set()

        def checked(expected: VerifyProvenance) -> None:
            missing.update(set(original) - {path.name for path in expected.directory.iterdir()})
            verify(
                Verify(
                    directory=expected.directory,
                    manifest_sha256=original['release.json'],
                    version=expected.version,
                    commit=expected.commit,
                )
            )

        def attest_subject(_options: VerifyProvenance, subject: str, bundle: str) -> None:
            self.assertEqual(Path(subject).read_bytes(), raw)
            if bundle.startswith(self.document['platforms'][0]['manifest_digest'].replace(':', '-')):
                raise subprocess.CalledProcessError(1, ['wrong-subject'])
            self.assertEqual(bundle, qualified['image']['index_digest'].replace(':', '-') + '.sigstore.jsonl')

        with (
            patch('hass_release.verify_provenance', side_effect=checked) as verify_subject,
            patch('hass_publish.command', side_effect=subprocess.CalledProcessError(1, ['wrong-signer'])) as signer,
            patch('hass_publish.image_manifest', return_value=raw),
            patch('hass_publish.attest', side_effect=attest_subject) as subject,
        ):
            qualify_rejections(options, qualified)
        self.assertEqual(verify_subject.call_count, 5)
        self.assertTrue({'release.json', 'SHA256SUMS'}.issubset(missing))
        signer.assert_called_once()
        self.assertEqual(subject.call_count, 2)
        self.assertEqual(original, {path.name: file_sha256(path) for path in candidate.iterdir()})
        with patch('hass_release.verify_provenance'), self.assertRaisesRegex(ValueError, 'accepted'):
            qualify_rejections(options, self.document)

    def test_wrong_subject_fetch_failure_cannot_count_as_a_provenance_rejection(self) -> None:
        options = VerifyProvenance(
            directory=self.directory / 'candidate',
            bundles=self.bundles,
            version='0.1.0',
            commit=self.document['source']['commit'],
        )
        with (
            patch('hass_release.verify_provenance', side_effect=ValueError('wrong commit')),
            patch('hass_publish.command', side_effect=subprocess.CalledProcessError(1, ['wrong-signer'])),
            patch('hass_publish.image_manifest', side_effect=subprocess.CalledProcessError(1, ['fetch failed'])),
            patch('hass_publish.attest') as attest,
            self.assertRaises(subprocess.CalledProcessError),
        ):
            qualify_rejections(options, self.document)
        attest.assert_not_called()

    def signed_draft(self) -> Path:
        draft = self.fixture.root / 'draft'
        shutil.copytree(self.directory / 'candidate', draft)
        for name in (
            'artifacts',
            'checksums',
            'acceptance',
            self.document['image']['index_digest'].replace(':', '-'),
            self.document['platforms'][0]['manifest_digest'].replace(':', '-'),
        ):
            (draft / (name + '.sigstore.jsonl')).write_text('Synthetic signed bundle\n')
        with tarfile.open(draft / 'haos-acceptance.tar.gz', 'w:gz') as archive:
            for name in EVIDENCE_FILES:
                path = self.evidence / name
                if not path.exists():
                    path.write_bytes(b'Synthetic public evidence')
                archive.add(path, arcname=name)
            archive.add(self.directory / 'staging.json', arcname='staging.json')
        return draft

    def test_incomplete_drafts_fail_before_any_registry_or_store_mutation(self) -> None:
        draft = self.signed_draft()
        original = {path.name: file_sha256(path) for path in draft.iterdir()}

        for missing in sorted(original):
            with self.subTest(missing=missing):

                def download(*args: str, cwd: Path | None = None, omitted: str = missing) -> str:
                    del cwd
                    if args[:3] == ('gh', 'release', 'view'):
                        return '{"isDraft":true}'
                    if args[:3] != ('gh', 'release', 'download'):
                        raise AssertionError('Incomplete draft reached an external mutation: ' + str(args))
                    destination = Path(args[args.index('--dir') + 1])
                    for path in draft.iterdir():
                        if path.name != omitted:
                            shutil.copyfile(path, destination / path.name)
                    return ''

                with (
                    patch('hass_publish.command', side_effect=download),
                    patch('hass_release.verify_provenance') as verified,
                    patch('hass_publish.promote_candidate') as promote,
                    self.assertRaises((ValueError, FileNotFoundError)),
                ):
                    promote_draft(
                        PromoteDraft(source=self.fixture.source, output=self.fixture.root / ('missing-' + missing))
                    )
                verified.assert_not_called()
                promote.assert_not_called()
        self.assertEqual({path.name: file_sha256(path) for path in draft.iterdir()}, original)

    def test_existing_signed_draft_is_promoted_without_building_an_image_or_replacing_assets(self) -> None:
        draft = self.signed_draft()
        repository = self.fixture.root / 'repository'
        repository.mkdir()
        (repository / 'repository.yaml').write_text('Synthetic repository\n')
        (repository / 'garmin_toolkit').mkdir()
        (repository / 'garmin_toolkit/config.yaml').write_text('arch: [amd64, aarch64]\n')
        calls: list[tuple[str, ...]] = []

        def downloaded(*args: str, cwd: Path | None = None) -> str:
            del cwd
            calls.append(args)
            if args[:3] == ('gh', 'release', 'view'):
                return json.dumps({'isDraft': is_draft})
            if args[:3] == ('gh', 'release', 'download'):
                destination = Path(args[args.index('--dir') + 1])
                for path in draft.iterdir():
                    shutil.copyfile(path, destination / path.name)
                return ''
            if args[:2] == ('nix', 'build'):
                self.assertTrue(args[2].endswith('#garmin-hass-app-repository'))
                return str(repository)
            raise AssertionError('Unexpected external action: ' + str(args))

        for is_draft in (True, False):
            with self.subTest(is_draft=is_draft):
                promoted = self.fixture.root / f'promoted-{is_draft}'
                with (
                    patch('hass_publish.command', side_effect=downloaded),
                    patch('hass_release.verify_provenance') as verified,
                    patch('hass_publish.attest'),
                    patch('hass_publish.promote_candidate') as promote,
                ):
                    promote_draft(PromoteDraft(source=self.fixture.source, output=promoted))
                verified.assert_called_once()
                promote.assert_called_once()
                self.assertEqual(
                    yaml.safe_load((promoted / 'repository/garmin_toolkit/config.yaml').read_text())['arch'], ['amd64']
                )
                self.assertFalse(
                    any(args[:3] in (('gh', 'release', 'create'), ('gh', 'release', 'upload')) for args in calls)
                )
                for entry in self.document['files']:
                    if entry['role'] in ('image_archive', 'sbom'):
                        self.assertTrue(
                            (promoted / 'native' / entry['name']).samefile(promoted / 'candidate' / entry['name'])
                        )

    def test_publication_and_ambiguous_push_failures_have_safe_rollback(self) -> None:
        real_git, real_command = hass_publish.git, hass_publish.command
        repository = self.directory / 'repository'
        repository.mkdir()
        config = repository / 'repository.yaml'
        for existing, failure in product((False, True), ('publish', 'push-before', 'push-after')):
            with self.subTest(existing=existing, failure=failure):
                remote = self.fixture.root / f'remote-{existing}-{failure}.git'
                subprocess.run(['git', 'init', '--bare', '-q', str(remote)], check=True)
                with patch('hass_publish.REMOTE', str(remote)):
                    config.write_text('previous metadata\n')
                    previous = push_metadata(repository, 'hass-apps', '') if existing else ''
                    config.write_text('new metadata\n')
                    (self.directory / 'promotion.json').unlink(missing_ok=True)
                    failed = False

                    def ambiguous(*args: str, cwd: Path | None = None, phase: str = failure) -> str:
                        nonlocal failed
                        store_push = args[0] == 'push' and any(arg.endswith(':refs/heads/hass-apps') for arg in args)
                        if store_push and phase == 'push-before' and not failed:
                            failed = True
                            raise subprocess.TimeoutExpired(args, 630)
                        result = real_git(*args, cwd=cwd)
                        if store_push and phase == 'push-after' and not failed:
                            failed = True
                            raise subprocess.TimeoutExpired(args, 630)
                        return result

                    def command(*args: str, cwd: Path | None = None, phase: str = failure) -> str:
                        if args[0] == 'git':
                            return real_command(*args, cwd=cwd)
                        if args[:2] == ('skopeo', 'list-tags'):
                            return '{"Tags":[]}'
                        self.assertTrue((self.directory / 'promotion.json').is_file())
                        if args[:3] == ('gh', 'release', 'edit') and phase == 'publish':
                            raise subprocess.TimeoutExpired(args, 630)
                        return ''

                    with (
                        patch('hass_publish.git', side_effect=ambiguous),
                        patch('hass_publish.command', side_effect=command),
                        patch('hass_publish.image_digest', return_value=self.document['image']['index_digest']),
                        self.assertRaises(subprocess.TimeoutExpired),
                    ):
                        promote_candidate(
                            StagePublication(
                                source=self.fixture.source, directory=self.directory, build=self.fixture.artifacts
                            ),
                            self.document,
                            {**self.staging, 'previous': previous},
                        )
                    self.assertEqual(branch_head('hass-apps') == previous, failure != 'push-after')
                    rollback_store(RollbackStore(directory=self.directory))
                    self.assertEqual(branch_head('hass-apps'), previous)
                    rollback_store(RollbackStore(directory=self.directory))

    def test_store_retry_reuses_exact_published_metadata_and_rejects_unrelated_changes(self) -> None:
        remote, repository = self.fixture.root / 'remote.git', self.directory / 'repository'
        subprocess.run(['git', 'init', '--bare', '-q', str(remote)], check=True)
        repository.mkdir()
        config = repository / 'repository.yaml'
        config.write_text('old metadata\n')
        prepared: list[tuple[str, str]] = []

        def before_push(commit: str, current: str, _checkout: Path) -> None:
            prepared.append((commit, current))

        with patch('hass_publish.REMOTE', str(remote)):
            previous = push_metadata(repository, 'hass-apps', '')
            config.write_text('new metadata\n')

            def guarded(commit: str, current: str, checkout: Path) -> tuple[str, str] | None:
                before_push(commit, current, checkout)
                return hass_publish.store().prepare(checkout, self.directory, current, commit)

            published = push_metadata(repository, 'hass-apps', previous, resume=True, before_push=guarded)
            retried = push_metadata(repository, 'hass-apps', previous, resume=True, before_push=before_push)
            self.assertEqual(retried, published)
            self.assertEqual(prepared, [(published, previous), (published, published)])
            config.write_text('unrelated metadata\n')
            unrelated = push_metadata(repository, 'hass-apps', published)
            config.write_text('new metadata\n')
            with self.assertRaisesRegex(ValueError, 'unrelated metadata'):
                push_metadata(repository, 'hass-apps', previous, resume=True, before_push=before_push)
            self.assertEqual(prepared, [(published, previous), (published, published)])
            with self.assertRaisesRegex(ValueError, 'unrelated metadata'):
                rollback_store(RollbackStore(directory=self.directory))
            self.assertEqual(branch_head('hass-apps'), unrelated)

    def test_rollback_before_any_mutation_needs_no_remote_access(self) -> None:
        with patch('hass_publish.branch_head') as head:
            rollback_store(RollbackStore(directory=self.directory))
        head.assert_not_called()

    def test_matching_advertisement_without_durable_qualification_is_refused(self) -> None:
        remote, repository = self.fixture.root / 'remote.git', self.directory / 'repository'
        subprocess.run(['git', 'init', '--bare', '-q', str(remote)], check=True)
        repository.mkdir()
        (repository / 'repository.yaml').write_text('existing unqualified metadata\n')
        with patch('hass_publish.REMOTE', str(remote)):
            published = push_metadata(repository, 'hass-apps', '')
            with self.assertRaisesRegex(ValueError, 'no durable qualification'):
                push_metadata(
                    repository,
                    'hass-apps',
                    '',
                    resume=True,
                    before_push=lambda commit, current, checkout: hass_publish.store().prepare(
                        checkout, self.directory, current, commit
                    ),
                )
            self.assertEqual(branch_head('hass-apps'), published)
            self.assertFalse((self.directory / 'promotion.json').exists())

    def test_staging_checks_preconditions_before_recovering_and_recording_the_baseline(self) -> None:
        remote, repository = self.fixture.root / 'remote.git', self.directory / 'repository'
        subprocess.run(['git', 'init', '--bare', '-q', str(remote)], check=True)
        config = repository / 'garmin_toolkit/config.yaml'
        config.parent.mkdir(parents=True)
        config.write_text('version: 0.1.0\n')
        real_command = hass_publish.command

        def command(*args: str, cwd: Path | None = None) -> str:
            return real_command(*args, cwd=cwd) if args[0] == 'git' else ''

        with patch('hass_publish.REMOTE', str(remote)):
            previous = push_metadata(repository, 'hass-apps', '')
            config.write_text('version: 0.2.0\n')
            advertised = push_metadata(
                repository,
                'hass-apps',
                previous,
                before_push=lambda commit, current, checkout: hass_publish.store().prepare(
                    checkout, self.directory, current, commit
                ),
            )
            occupied = push_metadata(repository, 'hass-candidate-123-2', '')
            fence = branch_head(hass_publish.FENCE)
            staged = self.fixture.root / 'new-staging'
            shutil.copytree(repository, staged / 'repository')
            options = StagePublication(source=self.fixture.source, directory=staged, build=self.fixture.artifacts)
            with (
                patch('hass_publish.checked_candidate', return_value=self.document),
                patch('hass_publish.verify_registry'),
                patch('hass_publish.image_digest', return_value=self.document['image']['index_digest']),
                patch('hass_publish.command', side_effect=command),
                patch.dict(os.environ, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1'),
            ):
                for run, attempt, error in (
                    ('', '1', 'actual GitHub workflow'),
                    ('123', '', 'actual GitHub workflow'),
                    ('123', '2', 'Candidate branch already exists'),
                ):
                    with self.subTest(run=run, attempt=attempt):
                        with (
                            patch.dict(os.environ, GITHUB_RUN_ID=run, GITHUB_RUN_ATTEMPT=attempt),
                            self.assertRaisesRegex(ValueError, error),
                        ):
                            stage_publication(options)
                        self.assertEqual(branch_head('hass-apps'), advertised)
                        self.assertEqual(branch_head(hass_publish.FENCE), fence)
                        self.assertEqual(branch_head('hass-candidate-123-2'), occupied)
                stage_publication(options)
            self.assertEqual(json.loads((staged / 'staging.json').read_bytes())['previous'], previous)
            self.assertEqual(branch_head('hass-apps'), previous)

    def test_cleanup_attempts_candidate_removal_without_hiding_failed_recovery(self) -> None:
        with (
            patch('hass_publish.rollback_store', side_effect=RuntimeError('recovery failed')),
            patch('hass_publish.cleanup_candidate_branch') as cleanup,
            self.assertRaisesRegex(RuntimeError, 'recovery failed'),
        ):
            cleanup_staging(CleanupStaging(directory=self.directory))
        cleanup.assert_called_once()

    def test_candidate_cleanup_survives_interruption_around_the_push_and_requires_a_durable_journal(self) -> None:
        remote = self.fixture.root / 'candidate-remote.git'
        subprocess.run(['git', 'init', '--bare', '-q', str(remote)], check=True)
        real_command, real_journal = hass_publish.command, hass_publish.write_journal
        for mode in ('before-push', 'lost-reply', 'journal-failure'):
            staged = self.fixture.root / mode
            config = staged / 'repository/garmin_toolkit/config.yaml'
            config.parent.mkdir(parents=True)
            config.write_text('version: 0.1.0\n')

            def journal(
                directory: Path,
                data: Object,
                *,
                replace: bool = False,
                name: str = 'promotion.json',
                mode: str = mode,
            ) -> None:
                if mode == 'journal-failure':
                    raise RuntimeError('journal unavailable')
                real_journal(directory, data, replace=replace, name=name)

            def command(*args: str, cwd: Path | None = None, mode: str = mode, staged: Path = staged) -> str:
                if args[0] != 'git':
                    return ''
                if 'push' not in args:
                    return real_command(*args, cwd=cwd)
                record = json.loads((staged / 'staging.json').read_bytes())
                self.assertIn(record['commit'] + ':refs/heads/' + record['branch'], args)
                if mode == 'before-push':
                    raise RuntimeError('interrupted before push')
                result = real_command(*args, cwd=cwd)
                if mode == 'lost-reply':
                    raise RuntimeError('interrupted after server accepted push')
                return result

            with (
                self.subTest(mode=mode),
                patch('hass_publish.REMOTE', str(remote)),
                patch('hass_publish.command', side_effect=command),
                patch('hass_publish.write_journal', side_effect=journal),
                patch('hass_publish.verify_registry'),
                patch('hass_publish.image_digest', return_value=self.document['image']['index_digest']),
                patch.dict(os.environ, GITHUB_RUN_ID='456', GITHUB_RUN_ATTEMPT='1'),
                self.assertRaisesRegex(RuntimeError, 'journal unavailable|interrupted'),
            ):
                hass_publish.stage_image(
                    StagePublication(source=self.fixture.source, directory=staged, build=self.fixture.artifacts),
                    '0.1.0',
                    self.document['image']['index_digest'],
                    '',
                )
            with patch('hass_publish.REMOTE', str(remote)), chdir(self.fixture.source):
                current = branch_head('hass-candidate-456-1')
                if mode == 'lost-reply':
                    self.assertTrue(current)
                else:
                    self.assertFalse(current)
                hass_publish.cleanup_candidate_branch(CleanupStaging(directory=staged))
                self.assertEqual(branch_head('hass-candidate-456-1'), '')
