"""Coordinate browser-owned actions and Supervisor lifecycle checks in disposable HAOS."""

import hashlib
import json
import subprocess
import time
from collections.abc import Callable
from pathlib import Path
from typing import cast

import haos_app_vm as vm
from haos_lifecycle import Lifecycle

ROOT = Path(__file__).resolve().parents[2]


def rows(snapshot: dict[str, object], key: str) -> list[dict[str, object]]:
    value = snapshot[key]
    if not isinstance(value, list) or not all(isinstance(row, dict) for row in cast('list[object]', value)):
        raise RuntimeError(f'Invalid {key} in the disposable storage oracle')
    return cast('list[dict[str, object]]', value)


def snapshot(key: Path, port: int, app_slug: str = 'local_garmin_toolkit') -> dict[str, object]:
    # The independent oracle reads a closed database, including its checkpointed
    # WAL. It never competes with the application's SQLite connections.
    vm.run(vm.ssh_command(key, port, f'ha apps stop {app_slug}'), timeout=90)
    probe = ROOT / 'infra/python/haos_storage_probe.py'
    result = subprocess.run(
        vm.ssh_command(
            key,
            port,
            f'docker exec -i hassio_supervisor python3 - /data/apps/data/{app_slug}',
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
    vm.run(vm.ssh_command(key, port, f'ha apps start {app_slug}'), timeout=120)
    vm.assert_started(key, port, app_slug)
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


def backup(key: Path, port: int, app_slug: str) -> str:
    slug = vm.guest_data(key, port, f'ha backups new --app {app_slug} --no-progress', timeout=600).get('slug')
    if not isinstance(slug, str) or not slug.isalnum():
        raise RuntimeError('Supervisor returned an invalid cold-backup slug')
    vm.assert_started(key, port, app_slug)
    return slug


def restore(key: Path, port: int, app_slug: str, slug: str) -> None:
    restored: object = json.loads(
        vm.run(
            vm.ssh_command(key, port, f'ha backups restore {slug} --app {app_slug} --no-progress --raw-json'),
            timeout=600,
        ).stdout
    )
    if not isinstance(restored, dict) or cast('dict[str, object]', restored).get('result') != 'ok':
        raise RuntimeError('Supervisor did not report a successful cold restore')
    vm.assert_started(key, port, app_slug)


def exercise(
    browser: subprocess.Popen[str],
    runtime: Path,
    evidence: Path,
    key: Path,
    port: int,
    app_slug: str = 'local_garmin_toolkit',
    *,
    reboot: Callable[[], dict[str, str]],
    lifecycle: Lifecycle | None = None,
) -> None:
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

    def persisted(phase: str, state: dict[str, object], hashes: dict[str, str]) -> None:
        command(phase)
        if snapshot(key, port, app_slug) != state or verify_downloads(state, evidence, phase) != hashes:
            raise RuntimeError(f'{phase} changed the populated state or ingress downloads')

    initial = snapshot(key, port, app_slug)
    if any(rows(initial, name) for name in ('profiles', 'routes', 'courses')):
        raise RuntimeError('The populated ingress workflow requires a fresh disposable app')
    command('populate')
    populated = snapshot(key, port, app_slug)
    if len(rows(populated, 'profiles')) != 1:
        raise RuntimeError('Profile creation did not persist exactly one profile')
    hashes = verify_downloads(populated, evidence, 'populated')
    previous_backup = None
    if lifecycle is not None:
        lifecycle.assert_image('baseline')
        previous_backup = backup(key, port, app_slug)
        lifecycle.upgrade()
        persisted('upgraded', populated, hashes)
    print('Restarting the disposable app through Supervisor...', flush=True)
    vm.run(vm.ssh_command(key, port, f'ha apps restart {app_slug}'), timeout=120)
    vm.assert_started(key, port, app_slug)
    persisted('restarted', populated, hashes)
    boot_ids = reboot()
    persisted('rebooted', populated, hashes)
    slug = backup(key, port, app_slug)
    command('mutate')
    mutated = snapshot(key, port, app_slug)
    if rows(mutated, 'courses') or len(rows(mutated, 'profiles')) != 2:
        raise RuntimeError('The post-backup mutation did not remove the Course and add a profile')
    restore(key, port, app_slug, slug)
    persisted('restored', populated, hashes)
    lifecycle_result = None
    if lifecycle is not None:
        assert previous_backup is not None
        command('mutate')
        lifecycle.reinstall_baseline(previous_backup)
        empty = snapshot(key, port, app_slug)
        if any(rows(empty, name) for name in ('profiles', 'routes', 'courses', 'artifacts')):
            raise RuntimeError('Uninstall/reinstall retained private app data')
        command('reinstalled')
        restore(key, port, app_slug, previous_backup)
        lifecycle.refresh_ingress()
        lifecycle.assert_image('baseline')
        persisted('recovered', populated, hashes)
        lifecycle.upgrade()
        persisted('recovered-upgraded', populated, hashes)
        lifecycle_result = {
            'upgrade': True,
            'recovery': True,
            'app_data_removed': True,
            'reinstall_empty': True,
            'backup_retained': True,
            'supervisor_restarted_after_restore': True,
            'baseline': 'metadata-only fixture; candidate filesystem; no previous public release',
            'migration_compatibility': False,
        }
    (evidence / 'workflow.json').write_text(
        json.dumps(
            {
                'passed': True,
                'app_slug': app_slug,
                'profile_count': 1,
                'route_count': 1,
                'course_count': 1,
                'downloads_sha256': hashes,
                'restart': True,
                'haos_reboot': boot_ids,
                'cold_restore': True,
                'post_backup_mutation_reverted': True,
                'browser_only_automation': True,
                'lifecycle': lifecycle_result,
            },
            indent=2,
        )
        + '\n'
    )
    print(
        f'Populated ingress, app restart, HAOS reboot, and cold backup/restore passed; evidence: {evidence}', flush=True
    )
