"""Supervisor transitions for a private metadata-only baseline and the unchanged CI candidate."""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

import haos_app_vm as vm
from haos_repository import publish_version
from json_data import Object, json_bytes, object_value, text


@dataclass(frozen=True)
class Lifecycle:
    runtime: Path
    evidence: Path
    key: Path
    port: int
    app_slug: str
    process: subprocess.Popen[str]
    overlay_limit: int

    def image(self, name: str) -> Object:
        images = object_value(json.loads((self.runtime / 'lifecycle-images.json').read_bytes()))
        if object_value(images['baseline'])['config_digest'] == object_value(images['candidate'])['config_digest']:
            raise RuntimeError('Lifecycle acceptance requires different baseline and candidate image configurations')
        (self.evidence / 'lifecycle-images.json').write_bytes(json_bytes(images))
        return object_value(images[name])

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

    def advertise(self, name: str) -> None:
        version = text(self.image(name)['version'])
        publish_version(self.runtime, version)
        vm.reload_store(self.key, self.port, self.app_slug)
        info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
        if info.get('version_latest') != version:
            raise RuntimeError('Supervisor did not discover the advertised lifecycle version')

    def upgrade(self) -> None:
        self.advertise('candidate')
        info = vm.guest_data(self.key, self.port, f'ha apps info {self.app_slug}')
        if info.get('update_available') is not True:
            raise RuntimeError('Supervisor must offer a real version update before acceptance')
        vm.run(vm.ssh_command(self.key, self.port, f'ha apps update {self.app_slug}'), timeout=600)
        self.assert_image('candidate')

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
