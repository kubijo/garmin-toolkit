"""Read a disposable app's selected database inside the Supervisor container."""

import json
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path
from typing import cast
from uuid import UUID


def collect(root: Path, scratch: Path) -> dict[str, object]:
    selection: object = json.loads((root / 'storage-selection.json').read_text())
    if not isinstance(selection, dict):
        raise ValueError('Invalid disposable storage selection')
    active = cast('dict[str, object]', selection).get('active')
    if active == 'Original':
        database = root / 'storage.sqlite3'
    elif isinstance(active, dict):
        value = cast('dict[str, object]', active).get('Generation')
        if not isinstance(value, str):
            raise ValueError('Invalid disposable storage generation')
        generation = str(UUID(value))
        database = root / 'storage-generations' / generation / 'storage.sqlite3'
    else:
        raise ValueError('Unknown disposable storage generation')
    # The caller stops the app before this coherent database/WAL copy. Rebuild
    # the shared-memory index privately instead of opening its existing -shm.
    with tempfile.TemporaryDirectory(prefix='garmin-storage-oracle.', dir=scratch) as directory:
        copied = Path(directory) / 'storage.sqlite3'
        shutil.copyfile(database, copied)
        wal = database.with_name(database.name + '-wal')
        if wal.is_file():
            shutil.copyfile(wal, copied.with_name(copied.name + '-wal'))
        return read_database(copied)


def read_database(database: Path) -> dict[str, object]:
    with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as connection:
        connection.row_factory = sqlite3.Row
        connection.execute('BEGIN')
        profiles = [dict(row) for row in connection.execute('SELECT id, display_name FROM users ORDER BY id')]
        routes = [
            dict(row)
            for row in connection.execute(
                'SELECT r.id, r.plan_id, r.name, r.sport, r.source_artifact_id '
                'FROM route_plan_revisions r JOIN route_plan_heads h ON h.revision_id = r.id ORDER BY r.id'
            )
        ]
        courses = [
            dict(row)
            for row in connection.execute('SELECT id, version, artifact_id FROM course_generations ORDER BY id')
        ]
        artifacts = [
            dict(row)
            for row in connection.execute(
                'SELECT a.id, hex(b.bytes) AS bytes FROM artifacts a '
                'JOIN artifact_blobs b ON a.digest = b.digest ORDER BY a.id'
            )
        ]
    return {'profiles': profiles, 'routes': routes, 'courses': courses, 'artifacts': artifacts}


if __name__ == '__main__':
    # This entrypoint runs inside the disposable guest's Supervisor container.
    print(json.dumps(collect(Path(sys.argv[1]), Path('/tmp'))))
