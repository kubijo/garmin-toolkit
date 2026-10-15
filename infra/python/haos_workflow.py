"""Coordinate browser-owned actions and Supervisor lifecycle checks in disposable HAOS."""

import hashlib
import json
import subprocess
import time
from pathlib import Path
from typing import cast

import haos_app_vm as vm

ROOT = Path(__file__).resolve().parents[2]


def rows(snapshot: dict[str, object], key: str) -> list[dict[str, object]]:
    value = snapshot[key]
    if not isinstance(value, list) or not all(isinstance(row, dict) for row in cast('list[object]', value)):
        raise RuntimeError(f'Invalid {key} in the disposable storage oracle')
    return cast('list[dict[str, object]]', value)


def snapshot(key: Path, port: int) -> dict[str, object]:
    # The independent oracle reads a closed database, including its checkpointed
    # WAL. It never competes with the application's SQLite connections.
    vm.run(vm.ssh_command(key, port, 'ha apps stop local_garmin_toolkit'), timeout=90)
    probe = ROOT / 'infra/python/haos_storage_probe.py'
    result = subprocess.run(
        vm.ssh_command(
            key,
            port,
            'docker exec -i hassio_supervisor python3 - /data/apps/data/local_garmin_toolkit',
        ),
        input=probe.read_text(),
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f'The disposable storage oracle failed: {result.stderr[-1000:]}')
    value: object = json.loads(result.stdout)
    if not isinstance(value, dict):
        raise RuntimeError('The disposable storage oracle returned invalid JSON')
    vm.run(vm.ssh_command(key, port, 'ha apps start local_garmin_toolkit'), timeout=120)
    vm.assert_started(key, port)
    return cast('dict[str, object]', value)


def verify_downloads(state: dict[str, object], evidence: Path, prefix: str) -> dict[str, str]:
    courses = rows(state, 'courses')
    routes = rows(state, 'routes')
    if len(courses) != 1 or len(routes) != 1 or courses[0]['version'] != 1:
        raise RuntimeError('Expected exactly one route and one first-version FIT Course')
    if routes[0]['name'] != 'Ingress loop' or routes[0]['sport'] != 'walking':
        raise RuntimeError('The reviewed route did not retain its name and sport')
    blobs = {str(artifact['id']): bytes.fromhex(str(artifact['bytes'])) for artifact in rows(state, 'artifacts')}
    hashes: dict[str, str] = {}
    for name, artifact in [('source.gpx', routes[0]['source_artifact_id']), ('course.fit', courses[0]['artifact_id'])]:
        actual = (evidence / f'{prefix}-{name}').read_bytes()
        if actual != blobs[str(artifact)]:
            raise RuntimeError(f'The {prefix} ingress download differs from the stored {name} artifact')
        hashes[name] = hashlib.sha256(actual).hexdigest()
    source = ROOT / 'crates/garmin-gpx/tests/fixtures/candidates.gpx'
    if (evidence / f'{prefix}-source.gpx').read_bytes() != source.read_bytes():
        raise RuntimeError('The ingress GPX download differs from the imported synthetic fixture')
    return hashes


def exercise(browser: subprocess.Popen[str], runtime: Path, evidence: Path, key: Path, port: int) -> None:
    request_id = 0

    def command(operation: str, *, timeout: int = 120) -> dict[str, object]:
        nonlocal request_id
        request_id += 1
        request = runtime / 'workflow-request.json'
        pending = request.with_suffix('.pending')
        pending.write_text(json.dumps({'id': request_id, 'operation': operation}))
        pending.replace(request)
        deadline = time.monotonic() + timeout
        response = runtime / 'workflow-response.json'
        while time.monotonic() < deadline:
            if browser.poll() is not None:
                raise RuntimeError(f'Workflow browser exited: {(evidence / "browser.log").read_text()[-3000:]}')
            if response.is_file():
                result: object = json.loads(response.read_text())
                if isinstance(result, dict):
                    result = cast('dict[str, object]', result)
                    if result.get('id') == request_id:
                        if result.get('error'):
                            raise RuntimeError(f'Browser workflow {operation} failed: {result["error"]}')
                        print(f'Ingress workflow: {operation} passed.', flush=True)
                        return result
            time.sleep(0.2)
        raise TimeoutError(f'Browser workflow {operation} timed out')

    initial = snapshot(key, port)
    if any(rows(initial, name) for name in ('profiles', 'routes', 'courses')):
        raise RuntimeError('The populated ingress workflow requires a fresh disposable app')
    command('populate')
    populated = snapshot(key, port)
    if len(rows(populated, 'profiles')) != 1:
        raise RuntimeError('Profile creation did not persist exactly one profile')
    hashes = verify_downloads(populated, evidence, 'populated')
    print('Restarting the disposable app through Supervisor...', flush=True)
    vm.run(vm.ssh_command(key, port, 'ha apps restart local_garmin_toolkit'), timeout=120)
    vm.assert_started(key, port)
    command('restarted')
    if snapshot(key, port) != populated or verify_downloads(populated, evidence, 'restarted') != hashes:
        raise RuntimeError('App restart changed the populated storage or ingress downloads')
    backup = vm.guest_data(key, port, 'ha backups new --app local_garmin_toolkit --no-progress', timeout=600)
    slug = backup.get('slug')
    if not isinstance(slug, str) or not slug.isalnum():
        raise RuntimeError('Supervisor returned an invalid cold-backup slug')
    vm.assert_started(key, port)
    command('mutate')
    mutated = snapshot(key, port)
    if rows(mutated, 'courses') or len(rows(mutated, 'profiles')) != 2:
        raise RuntimeError('The post-backup mutation did not remove the Course and add a profile')
    restored: object = json.loads(
        vm.run(
            vm.ssh_command(key, port, f'ha backups restore {slug} --app local_garmin_toolkit --no-progress --raw-json'),
            timeout=600,
        ).stdout
    )
    if not isinstance(restored, dict) or cast('dict[str, object]', restored).get('result') != 'ok':
        raise RuntimeError('Supervisor did not report a successful cold restore')
    vm.assert_started(key, port)
    command('restored')
    recovered = snapshot(key, port)
    if recovered != populated or verify_downloads(recovered, evidence, 'restored') != hashes:
        raise RuntimeError('Cold restore did not recover the exact populated state and downloaded artifacts')
    (evidence / 'workflow.json').write_text(
        json.dumps(
            {
                'passed': True,
                'profile_count': 1,
                'route_count': 1,
                'course_count': 1,
                'downloads_sha256': hashes,
                'restart': True,
                'cold_restore': True,
                'post_backup_mutation_reverted': True,
                'browser_only_automation': True,
            },
            indent=2,
        )
        + '\n'
    )
    print(f'Populated ingress, restart, and cold backup/restore passed; evidence: {evidence}', flush=True)
