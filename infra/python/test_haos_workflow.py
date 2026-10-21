"""Check selected-storage inspection and byte-exact ingress acceptance oracles."""

import json
import sqlite3
import subprocess
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import Mock, patch
from uuid import uuid4

import haos_app_session as session
import haos_app_vm as vm
import haos_storage_probe as probe
import haos_workflow as workflow
from haos_lifecycle import Lifecycle


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


if __name__ == '__main__':
    unittest.main()
