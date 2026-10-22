"""Supervisor transitions from a fixture or authenticated published baseline to the unchanged CI candidate."""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

import haos_app_vm as vm
from haos_recovery import Recovery, update_failure
from haos_repository import publish_version
from haos_withdrawal import Withdrawal
from json_data import Object, file_sha256, json_bytes, object_rows, object_value, read_json, text


@dataclass(frozen=True)
class Lifecycle:
    runtime: Path
    evidence: Path
    key: Path
    port: int
    app_slug: str
    process: subprocess.Popen[str]
    overlay_limit: int
    release_assets: tuple[tuple[Path, str], ...] = ()

    def image(self, name: str) -> Object:
        images = object_value(json.loads((self.runtime / 'lifecycle-images.json').read_bytes()))
        if object_value(images['baseline'])['config_digest'] == object_value(images['candidate'])['config_digest']:
            raise RuntimeError('Lifecycle acceptance requires different baseline and candidate image configurations')
        (self.evidence / 'lifecycle-images.json').write_bytes(json_bytes(images))
        return object_value(images[name])

    def coverage(self) -> Object:
        published = self.runtime / 'published-baseline.json'
        if not published.is_file():
            return {
                'baseline': 'metadata-only fixture; candidate filesystem; no previous public release',
                'migration_compatibility': False,
            }
        identity = object_value(json.loads(published.read_bytes()))
        baseline, candidate = self.image('baseline'), self.image('candidate')
        if identity.get('signatures_verified') is not True or any(
            identity.get(key) != baseline.get(key) for key in ('archive_sha256', 'config_digest', 'manifest_digest')
        ):
            raise RuntimeError('Published upgrade baseline differs from its authenticated release image')
        return {
            'baseline': 'published release ' + text(identity['release']),
            'published_baseline': identity,
            'candidate_version': candidate['version'],
            'candidate_archive_version': candidate['archive_version'],
            'migration_compatibility': True,
        }

    def assert_image(self, name: str) -> Object:
        expected = self.image(name)
        info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
        if info.get('version') != expected['version']:
            raise RuntimeError('Supervisor did not install the expected lifecycle version')
        vm.assert_started(self.key, self.port, self.app_slug)
        identity = vm.assert_installed_image(
            self.key,
            self.port,
            self.app_slug,
            reference=text(expected['image_reference']),
            digest=text(expected['registry_digest']),
            config_digest=text(expected['config_digest']),
            architecture=text(expected['architecture']),
        )
        if identity['manifest_digest'] != expected['manifest_digest']:
            raise RuntimeError('Lifecycle installation differs from the native OCI manifest')
        installed = object_value(json.loads((self.runtime / 'installation.json').read_bytes()))
        installed.update(expected)
        installed.update(identity)
        encoded = json_bytes(installed)
        for path in (
            self.runtime / 'installation.json',
            self.evidence / 'installation.json',
            self.evidence / f'{name}-installation.json',
        ):
            path.write_bytes(encoded)
        return identity

    def recovery_images(self) -> Object:
        coverage = self.coverage()
        if coverage.get('migration_compatibility') is not True:
            raise RuntimeError('Installation recovery requires an authenticated published baseline')
        identity = object_value(read_json(self.runtime / 'published-candidate.json'))
        candidate = self.image('candidate')
        if (
            identity.get('signatures_verified') is not True
            or candidate['version'] != candidate['archive_version']
            or identity.get('version') != candidate['version']
            or any(
                identity.get(key) != candidate.get(key)
                for key in ('archive_sha256', 'config_digest', 'manifest_digest')
            )
        ):
            raise RuntimeError('Recovery candidate differs from its authenticated published image')
        if not self.release_assets or any(file_sha256(path) != digest for path, digest in self.release_assets):
            raise RuntimeError('Recovery changed or lost authenticated release inputs')
        return {
            **coverage,
            'published_candidate': identity,
            'release_assets_preserved': True,
        }

    def recovery_coverage(self) -> Object:
        return {
            **self.recovery_images(),
            'supervisor_update_failure': object_value(read_json(self.evidence / 'recovery-update-failure.json')),
        }

    def advertise(self, name: str) -> None:
        version = text(self.image(name)['version'])
        if (self.runtime / 'withdrawal-state.json').is_file():
            Withdrawal(self.runtime, self.evidence).replace(version)
        elif (self.runtime / 'recovery-state.json').is_file():
            Recovery(self.runtime, self.evidence).advertise('retry')
        else:
            publish_version(self.runtime, version)
        vm.reload_store(self.key, self.port, self.app_slug)
        info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
        if info.get('version_latest') != version:
            raise RuntimeError('Supervisor did not discover the advertised lifecycle version')

    def withdraw(self) -> None:
        version = text(self.image('baseline')['version'])
        self.assert_image('baseline')
        Withdrawal(self.runtime, self.evidence).withdraw(version)
        vm.reload_store(self.key, self.port, self.app_slug, present=False)
        self.assert_image('baseline')
        for name, command in (
            ('withdrawn-store', 'ha store info'),
            ('withdrawn-app', f'ha apps info {self.app_slug}'),
        ):
            (self.evidence / (name + '.json')).write_bytes(json_bytes(vm.guest_data(self.key, self.port, command)))

    def upgrade(self) -> None:
        self.advertise('candidate')
        info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
        if info.get('update_available') is not True:
            raise RuntimeError('Supervisor must offer a real version update before acceptance')
        vm.run(vm.ssh_command(self.key, self.port, f'ha apps update {self.app_slug}'), timeout=600)
        self.assert_image('candidate')

    def recover_installation(self) -> None:
        self.recovery_images()
        fixture = Recovery(self.runtime, self.evidence)
        image = text(self.image('candidate')['image_reference'])
        # A cached candidate would bypass the registry failure.
        vm.run(vm.ssh_command(self.key, self.port, '! docker image inspect ' + image + ' >/dev/null 2>&1'))
        fixture.prepare(text(self.image('candidate')['version']))
        try:
            fixture.advertise('failed')
            vm.reload_store(self.key, self.port, self.app_slug)
            info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
            if (
                info.get('version_latest') != self.image('candidate')['version']
                or info.get('update_available') is not True
            ):
                raise RuntimeError('Supervisor did not offer the recovery candidate before failure injection')
            try:
                vm.run(vm.ssh_command(self.key, self.port, 'docker stop garmin-test-registry'), timeout=30)
                registries = object_rows(
                    json.loads(
                        vm.run(
                            vm.ssh_command(self.key, self.port, 'docker container inspect garmin-test-registry')
                        ).stdout
                    )
                )
                if len(registries) != 1:
                    raise RuntimeError('Recovery requires exactly one fixture registry')
                registry_state = object_value(registries[0]['State'])
                (self.evidence / 'recovery-registry-state.json').write_bytes(json_bytes(registry_state))
                if registry_state.get('Running') is not False or registry_state.get('Status') != 'exited':
                    raise RuntimeError('Recovery fixture registry did not stop before the update')
                result = subprocess.run(
                    vm.ssh_command(self.key, self.port, f'ha apps update {self.app_slug} --raw-json'),
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=600,
                )
                (self.evidence / 'recovery-update.json').write_bytes(
                    json_bytes({'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
                )
                failure = {
                    **update_failure(result, self.app_slug),
                    'candidate_image': image,
                    'registry_state': registry_state,
                }
                (self.evidence / 'recovery-update-failure.json').write_bytes(json_bytes(failure))
            finally:
                vm.run(vm.ssh_command(self.key, self.port, 'docker start garmin-test-registry'), timeout=30)
        finally:
            fixture.rollback()
        vm.reload_store(self.key, self.port, self.app_slug)
        info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
        if info.get('version_latest') != self.image('baseline')['version'] or info.get('update_available') is not False:
            raise RuntimeError('Supervisor did not refresh the recovered baseline advertisement')
        self.assert_image('baseline')

    def reinstall_baseline(self, backup: str) -> None:
        vm.run(vm.ssh_command(self.key, self.port, f'ha apps uninstall {self.app_slug}'), timeout=180)
        vm.run(
            vm.ssh_command(
                self.key,
                self.port,
                f'docker exec hassio_supervisor test ! -e /data/apps/data/{self.app_slug} '
                f'&& ! docker container inspect app_{self.app_slug} >/dev/null 2>&1',
            )
        )
        vm.guest_data(self.key, self.port, f'ha backups info {backup}')
        self.advertise('baseline')
        vm.run(vm.ssh_command(self.key, self.port, f'ha apps install {self.app_slug}'), timeout=600)
        vm.run(vm.ssh_command(self.key, self.port, f'ha apps start {self.app_slug}'), timeout=120)
        self.assert_image('baseline')

    def refresh_ingress(self) -> None:
        # Supervisor 2026.10.2 restores an old token without refreshing its ingress routing map.
        print('Restarting Supervisor to refresh restored ingress routing...', flush=True)
        vm.run(vm.ssh_command(self.key, self.port, 'ha supervisor restart'), timeout=90)
        vm.wait_for_supervisor(
            self.process,
            self.runtime / 'overlay.qcow2',
            self.key,
            self.port,
            120,
            overlay_limit=self.overlay_limit,
        )
        vm.assert_started(self.key, self.port, self.app_slug)
