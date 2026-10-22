"""Check selected-storage inspection and byte-exact ingress acceptance oracles."""

import json
import sqlite3
import subprocess
import tempfile
import unittest
from contextlib import closing
from dataclasses import replace
from pathlib import Path
from unittest.mock import Mock, patch
from uuid import uuid4

import haos_app_session as session
import haos_app_vm as vm
import haos_storage_probe as probe
import haos_workflow as workflow
from haos_lifecycle import Lifecycle
from haos_platform import PLATFORMS
from haos_recovery import update_failure
from json_data import file_sha256


class WorkflowTests(unittest.TestCase):
    def setUp(self) -> None:
        vm.SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='haos-workflow-tests.', dir=vm.SCRATCH_ROOT)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def lifecycle(self) -> Lifecycle:
        return Lifecycle(self.root, self.root, self.root / 'key', 2222, 'test_app', Mock(spec=subprocess.Popen), 1024)

    def database(self, directory: Path) -> Path:
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / 'storage.sqlite3'
        with closing(sqlite3.connect(path)) as connection, connection:
            for migration in sorted((workflow.ROOT / 'crates/garmin-storage/migrations').glob('*.sql')):
                connection.executescript(migration.read_text())
        return path

    def test_probe_reads_the_selected_generation_without_changing_either_database(self) -> None:
        original = self.database(self.root)
        generation = str(uuid4())
        selected = self.database(self.root / 'storage-generations' / generation)
        with closing(sqlite3.connect(selected)) as connection, connection:
            connection.execute("INSERT INTO users (id, role, display_name) VALUES ('test-owner', 'owner', 'Selected')")
        (self.root / 'storage-selection.json').write_text(
            json.dumps({'active': {'Generation': generation}, 'rollback': 'Original'})
        )
        before = [path.read_bytes() for path in [original, selected]]
        state = probe.collect(self.root, self.root)
        self.assertEqual(state['profiles'], [{'id': 'test-owner', 'display_name': 'Selected'}])
        self.assertEqual(state['courses'], [])
        self.assertEqual([path.read_bytes() for path in [original, selected]], before)

    def test_probe_rejects_invalid_selection_and_does_not_create_a_database(self) -> None:
        for active in [None, {'Generation': '../other'}, {'Generation': 123}]:
            with self.subTest(active=active):
                (self.root / 'storage-selection.json').write_text(json.dumps({'active': active}))
                with self.assertRaises(ValueError):
                    probe.collect(self.root, self.root)
        self.assertEqual(list(self.root.glob('*.sqlite3')), [])

    def test_probe_closes_its_database_on_success_and_query_failure(self) -> None:
        for broken in (False, True):
            with self.subTest(broken=broken):
                database = self.database(self.root / str(broken))
                connection = sqlite3.connect(database)
                self.addCleanup(connection.close)
                if broken:
                    connection.execute('DROP TABLE users')
                    connection.commit()
                with patch.object(probe.sqlite3, 'connect', return_value=connection):
                    if broken:
                        with self.assertRaises(sqlite3.OperationalError):
                            probe.read_database(database)
                    else:
                        probe.read_database(database)
                with self.assertRaises(sqlite3.ProgrammingError):
                    connection.execute('SELECT 1')

    def test_probe_keeps_committed_wal_data_without_reusing_shared_memory(self) -> None:
        database = self.database(self.root)
        (self.root / 'storage-selection.json').write_text(json.dumps({'active': 'Original'}))
        with closing(sqlite3.connect(database)) as connection, connection:
            connection.execute('PRAGMA journal_mode=WAL')
            connection.execute("INSERT INTO users (id, role, display_name) VALUES ('test-owner', 'owner', 'WAL')")
            connection.commit()
            self.assertTrue(database.with_name(database.name + '-wal').is_file())
            state = probe.collect(self.root, self.root)
            self.assertEqual(state['profiles'], [{'id': 'test-owner', 'display_name': 'WAL'}])
        self.assertEqual(list(self.root.glob('garmin-storage-oracle.*')), [])

    def test_download_oracle_rejects_changed_course_bytes_and_changed_source(self) -> None:
        source = (workflow.ROOT / 'crates/garmin-gpx/tests/fixtures/candidates.gpx').read_bytes()
        course = b'synthetic-course-download'
        state: dict[str, object] = {
            'routes': [{'name': 'Ingress loop', 'sport': 'walking', 'source_artifact_id': 'source'}],
            'courses': [{'version': 1, 'artifact_id': 'course'}],
            'artifacts': [{'id': 'source', 'bytes': source.hex()}, {'id': 'course', 'bytes': course.hex()}],
        }
        (self.root / 'check-source.gpx').write_bytes(source)
        target = self.root / 'check-course.fit'
        target.write_bytes(course)
        hashes = workflow.verify_downloads(state, self.root, 'check')
        self.assertEqual(set(hashes), {'source.gpx', 'course.fit'})
        target.write_bytes(course + b'corrupt')
        with self.assertRaisesRegex(RuntimeError, 'differs from the stored course.fit'):
            workflow.verify_downloads(state, self.root, 'check')
        target.write_bytes(course)
        (self.root / 'check-source.gpx').write_bytes(b'not-the-imported-gpx')
        with self.assertRaisesRegex(RuntimeError, 'differs from the stored source.gpx'):
            workflow.verify_downloads(state, self.root, 'check')

    def test_populated_check_cannot_skip_browser_verification(self) -> None:
        with self.assertRaisesRegex(ValueError, 'requires browser verification'):
            session.run_session(session.Check(app_archive=Path('not-used'), verify_browser=False))

    def test_lifecycle_requires_a_generated_repository_and_native_receipt(self) -> None:
        for repository, receipt in ((None, None), (self.root, None), (None, self.root / 'native-build.json')):
            with (
                self.subTest(repository=repository, receipt=receipt),
                self.assertRaisesRegex(ValueError, 'repository and native build receipt'),
            ):
                session.run_session(
                    session.Check(
                        app_archive=Path('not-used'), lifecycle=True, repository=repository, build_receipt=receipt
                    )
                )

    def test_withdrawal_requires_local_metadata_and_receipt_before_starting_any_resources(self) -> None:
        for repository, receipt in ((None, None), (self.root, None), (None, self.root / 'native-build.json')):
            with (
                self.subTest(repository=repository, receipt=receipt),
                self.assertRaisesRegex(ValueError, 'repository and native build receipt'),
            ):
                session.run_session(
                    session.Check(
                        app_archive=Path('not-used'), withdrawal=True, repository=repository, build_receipt=receipt
                    )
                )
        with self.assertRaisesRegex(ValueError, 'Select one acceptance mode'):
            session.run_session(session.Check(app_archive=Path('not-used'), withdrawal=True, lifecycle=True))

    def test_lifecycle_rejects_identical_baseline_and_candidate_images(self) -> None:
        (self.root / 'lifecycle-images.json').write_text(
            json.dumps(
                {
                    'baseline': {'config_digest': 'same'},
                    'candidate': {'config_digest': 'same'},
                }
            )
        )
        lifecycle = self.lifecycle()
        with self.assertRaisesRegex(RuntimeError, 'different baseline and candidate'):
            lifecycle.image('candidate')

    def test_lifecycle_upgrade_cannot_pass_without_an_available_update(self) -> None:
        lifecycle = self.lifecycle()
        with (
            patch.object(Lifecycle, 'advertise'),
            patch.object(vm, 'guest_data', return_value={'update_available': False}),
            patch.object(vm, 'run') as run,
            self.assertRaisesRegex(RuntimeError, 'real version update'),
        ):
            lifecycle.upgrade()
        run.assert_not_called()

    def test_fixture_cannot_claim_published_binary_migration_coverage(self) -> None:
        self.assertIs(self.lifecycle().coverage()['migration_compatibility'], False)

    def test_published_coverage_requires_the_authenticated_baseline_identity(self) -> None:
        baseline = {'config_digest': 'old', 'manifest_digest': 'old-manifest', 'archive_sha256': 'archive'}
        candidate = {'config_digest': 'new', 'version': '0.1.2-dev', 'archive_version': '0.1.1'}
        (self.root / 'lifecycle-images.json').write_text(json.dumps({'baseline': baseline, 'candidate': candidate}))
        identity = {**baseline, 'release': 'v0.1.1', 'signatures_verified': True}
        path = self.root / 'published-baseline.json'
        path.write_text(json.dumps(identity))
        coverage = self.lifecycle().coverage()
        self.assertIs(coverage['migration_compatibility'], True)
        self.assertEqual(coverage['candidate_version'], '0.1.2-dev')
        self.assertEqual(coverage['candidate_archive_version'], '0.1.1')
        for change in ({'signatures_verified': False}, {'manifest_digest': 'different'}):
            path.write_text(json.dumps({**identity, **change}))
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, 'authenticated release image'):
                self.lifecycle().coverage()

    def test_published_baseline_requires_lifecycle_acceptance(self) -> None:
        with self.assertRaisesRegex(ValueError, 'require lifecycle acceptance'):
            session.run_session(session.Check(app_archive=Path('unused'), baseline_release=self.root))

    def test_reinstall_refuses_retained_private_data(self) -> None:
        lifecycle = self.lifecycle()
        with (
            patch.object(
                vm, 'run', side_effect=[subprocess.CompletedProcess([], 0), RuntimeError('private data remains')]
            ),
            patch.object(Lifecycle, 'advertise') as advertise,
            self.assertRaisesRegex(RuntimeError, 'private data remains'),
        ):
            lifecycle.reinstall_baseline('backup123')
        advertise.assert_not_called()

    def test_interrupted_acceptance_cannot_report_success(self) -> None:
        with patch('haos_app_session.run_session', side_effect=session.SessionStopped):
            self.assertEqual(session.main(session.Check(app_archive=Path('not-used'))), 143)

    def test_recovery_requires_two_published_releases_and_disallows_version_aliases_and_other_modes(self) -> None:
        for baseline, candidate, repository in (
            (None, self.root, self.root),
            (self.root, None, self.root),
            (self.root, self.root, None),
        ):
            with (
                self.subTest(baseline=baseline, candidate=candidate, repository=repository),
                self.assertRaisesRegex(ValueError, 'both signed published releases'),
            ):
                session.run_session(
                    session.Check(
                        app_archive=Path('unused'),
                        recovery=True,
                        baseline_release=baseline,
                        candidate_release=candidate,
                        repository=repository,
                    )
                )
        complete = session.Check(
            app_archive=Path('unused'),
            recovery=True,
            baseline_release=self.root,
            candidate_release=self.root,
            repository=self.root,
        )
        for change in ({'lifecycle': True}, {'withdrawal': True}, {'candidate_version': '0.1.3'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                session.run_session(replace(complete, **change))
        with self.assertRaisesRegex(ValueError, 'requires recovery acceptance'):
            session.run_session(session.Check(app_archive=Path('unused'), candidate_release=self.root))

    def test_update_failure_uses_structured_fields_and_rejects_transport_errors(self) -> None:
        slug = 'test_garmin_toolkit'
        response = {
            'result': 'error',
            'error_key': 'app_unknown_error',
            'extra_fields': {'app': slug},
            'message': 'An unknown error occurred with the app',
        }
        for message in ('An unknown error occurred with the app', 'Aktualizace aplikace selhala.'):
            body = {**response, 'message': message}
            self.assertEqual(
                update_failure(subprocess.CompletedProcess([], 0, json.dumps(body)), slug)['response'], body
            )
        for code, body in (
            (1, response),
            (-15, response),
            (2, response),
            (255, response),
            (0, {**response, 'result': 'ok'}),
            (0, {**response, 'error_key': 'system_not_ready_error'}),
            (0, {**response, 'error_key': 'docker_no_space_on_device'}),
            (0, {**response, 'error_key': None}),
            (0, {**response, 'extra_fields': {'app': 'another_app'}}),
        ):
            with self.subTest(code=code, body=body), self.assertRaises(RuntimeError):
                update_failure(subprocess.CompletedProcess([], code, json.dumps(body)), slug)
        for raw in (
            'not a Supervisor response',
            '[]',
            '{"result":"error","result":"ok"}',
            json.dumps({**response, 'extra_fields': None}),
            json.dumps({**response, 'message': None}),
        ):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                update_failure(subprocess.CompletedProcess([], 0, raw), slug)

    def test_signed_recovery_rejects_another_archive_and_nonincreasing_releases_before_startup(self) -> None:
        archive = self.root / 'candidate.oci.tar'
        for selected, version in ((self.root / 'another.oci.tar', '0.1.2'), (archive, '0.1.0')):
            with (
                self.subTest(selected=selected, version=version),
                patch.object(vm, 'validate_options', return_value=PLATFORMS['amd64']),
                patch(
                    'haos_app_session.published_baseline',
                    side_effect=[
                        (self.root / 'baseline.oci.tar', {'version': '0.1.1'}),
                        (selected, {'version': version}),
                    ],
                ),
                patch('haos_app_session.session_scratch') as scratch,
                self.assertRaises(ValueError),
            ):
                session.run_session(
                    session.Check(
                        app_archive=archive,
                        recovery=True,
                        repository=self.root,
                        baseline_release=self.root / 'baseline',
                        candidate_release=self.root / 'candidate',
                    )
                )
            scratch.assert_not_called()

    def test_failed_supervisor_requests_restore_registry_and_roll_back_metadata(self) -> None:
        lifecycle = self.lifecycle()
        image = {'version': '0.1.2', 'image_reference': 'localhost:5000/garmin-toolkit-hass:0.1.2'}
        for failure in (
            subprocess.TimeoutExpired('owned Supervisor update', 600),
            subprocess.CompletedProcess([], 0, '{"result":"error","message":"unrelated error"}'),
        ):
            with (
                self.subTest(failure=failure),
                patch.object(Lifecycle, 'recovery_images'),
                patch.object(Lifecycle, 'image', return_value=image),
                patch('haos_lifecycle.Recovery', autospec=True) as fixture,
                patch.object(vm, 'reload_store'),
                patch.object(vm, 'guest_data', return_value={'version_latest': '0.1.2', 'update_available': True}),
                patch.object(
                    vm,
                    'run',
                    return_value=subprocess.CompletedProcess([], 0, '[{"State":{"Running":false,"Status":"exited"}}]'),
                ) as run,
                patch('haos_lifecycle.subprocess.run') as update,
            ):
                if isinstance(failure, subprocess.TimeoutExpired):
                    update.side_effect = failure
                else:
                    update.return_value = failure
                with self.assertRaises((RuntimeError, subprocess.TimeoutExpired)):
                    lifecycle.recover_installation()
                self.assertEqual(run.call_args.args[0][-1], 'docker start garmin-test-registry')
                fixture.return_value.rollback.assert_called_once()
                self.assertFalse((self.root / 'recovery-update-failure.json').exists())
                if isinstance(failure, subprocess.CompletedProcess):
                    self.assertEqual(
                        json.loads((self.root / 'recovery-update.json').read_text())['stdout'], failure.stdout
                    )

    def test_recovery_rejects_a_registry_that_did_not_stop(self) -> None:
        with (
            patch.object(Lifecycle, 'recovery_images'),
            patch.object(
                Lifecycle,
                'image',
                return_value={'version': '0.1.2', 'image_reference': 'localhost:5000/garmin-toolkit-hass:0.1.2'},
            ),
            patch('haos_lifecycle.Recovery', autospec=True) as fixture,
            patch.object(vm, 'reload_store'),
            patch.object(vm, 'guest_data', return_value={'version_latest': '0.1.2', 'update_available': True}),
            patch.object(
                vm,
                'run',
                return_value=subprocess.CompletedProcess([], 0, '[{"State":{"Running":true,"Status":"running"}}]'),
            ) as run,
            patch('haos_lifecycle.subprocess.run') as update,
            self.assertRaisesRegex(RuntimeError, 'did not stop'),
        ):
            self.lifecycle().recover_installation()
        update.assert_not_called()
        fixture.return_value.rollback.assert_called_once()
        self.assertEqual(run.call_args.args[0][-1], 'docker start garmin-test-registry')
        self.assertFalse((self.root / 'recovery-update-failure.json').exists())

    def test_signed_recovery_rejects_changed_candidate_identity_and_retained_input_bytes(self) -> None:
        baseline = {
            'version': '0.1.1',
            'config_digest': 'old',
            'manifest_digest': 'old-manifest',
            'archive_sha256': 'old-archive',
        }
        candidate = {
            'version': '0.1.2',
            'archive_version': '0.1.2',
            'config_digest': 'new',
            'manifest_digest': 'new-manifest',
            'archive_sha256': 'new-archive',
        }
        (self.root / 'lifecycle-images.json').write_text(json.dumps({'baseline': baseline, 'candidate': candidate}))
        (self.root / 'published-baseline.json').write_text(
            json.dumps({**baseline, 'release': 'v0.1.1', 'signatures_verified': True})
        )
        identity = {**candidate, 'release': 'v0.1.2', 'signatures_verified': True}
        path = self.root / 'published-candidate.json'
        path.write_text(json.dumps(identity))
        retained = self.root / 'authenticated-input'
        retained.write_bytes(b'Synthetic authenticated release input')
        lifecycle = replace(self.lifecycle(), release_assets=((retained, file_sha256(retained)),))
        self.assertIs(lifecycle.recovery_images()['release_assets_preserved'], True)
        for change in ({'signatures_verified': False}, {'manifest_digest': 'different'}):
            path.write_text(json.dumps({**identity, **change}))
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, 'authenticated published image'):
                lifecycle.recovery_images()
        path.write_text(json.dumps(identity))
        retained.write_bytes(b'Changed release input')
        with self.assertRaisesRegex(RuntimeError, 'authenticated release inputs'):
            lifecycle.recovery_images()


if __name__ == '__main__':
    unittest.main()
