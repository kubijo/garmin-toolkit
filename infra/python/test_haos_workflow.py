"""Check selected-storage inspection and byte-exact ingress acceptance oracles."""

import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from uuid import uuid4

import haos_app_session as session
import haos_app_vm as vm
import haos_storage_probe as probe
import haos_workflow as workflow


class WorkflowTests(unittest.TestCase):
    def setUp(self) -> None:
        vm.SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='haos-workflow-tests.', dir=vm.SCRATCH_ROOT)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def database(self, directory: Path) -> Path:
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / 'storage.sqlite3'
        with sqlite3.connect(path) as connection:
            for migration in sorted((workflow.ROOT / 'crates/garmin-storage/migrations').glob('*.sql')):
                connection.executescript(migration.read_text())
        return path

    def test_probe_reads_the_selected_generation_without_changing_either_database(self) -> None:
        original = self.database(self.root)
        generation = str(uuid4())
        selected = self.database(self.root / 'storage-generations' / generation)
        with sqlite3.connect(selected) as connection:
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

    def test_probe_keeps_committed_wal_data_without_reusing_shared_memory(self) -> None:
        database = self.database(self.root)
        (self.root / 'storage-selection.json').write_text(json.dumps({'active': 'Original'}))
        with sqlite3.connect(database) as connection:
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

    def test_interrupted_acceptance_cannot_report_success(self) -> None:
        with patch('haos_app_session.run_session', side_effect=session.SessionStopped):
            self.assertEqual(session.main(session.Check(app_archive=Path('not-used'))), 143)


if __name__ == '__main__':
    unittest.main()
