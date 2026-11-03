"""Local orchestration of a dev install through individual SSH and Supervisor CLI commands."""

import json
import shlex
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Protocol

from cli_output import TerminalOutput, child_environment
from json_data import Object, object_value, text

DEV_SLUG = 'local_garmin_toolkit-dev'


class Transport(Protocol):
    def execute(self, *arguments: str, timeout: int = 120) -> str: ...

    def upload(self, archive: Path, destination: str) -> None: ...


@dataclass(frozen=True)
class SshTransport:
    host: str

    def execute(self, *arguments: str, timeout: int = 120) -> str:
        result = subprocess.run(
            ['ssh', '-o', 'BatchMode=yes', self.host, shlex.join(arguments)],
            env=child_environment(),
            check=True,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return result.stdout

    def upload(self, archive: Path, destination: str) -> None:
        subprocess.run(
            ['scp', '-o', 'BatchMode=yes', str(archive), f'{self.host}:{destination}'],
            env=child_environment(),
            check=True,
            timeout=600,
        )


def optional_text(value: object) -> str | None:
    return None if value is None else text(value)


@dataclass(frozen=True)
class AppInfo:
    slug: str
    state: str
    version: str | None
    latest: str | None
    available: bool

    @property
    def installed(self) -> bool:
        return self.version is not None

    @classmethod
    def parse(cls, data: Object) -> AppInfo:
        return cls(
            slug=text(data.get('slug')),
            state=text(data.get('state')),
            version=optional_text(data.get('version')),
            latest=optional_text(data.get('version_latest')),
            available=data.get('available') is True,
        )


@dataclass
class Installer:
    transport: Transport
    output: TerminalOutput
    stage: str = 'preparing deployment'

    def step(self, stage: str) -> None:
        self.stage = stage
        self.output.console.print(stage)

    def ha(self, *arguments: str, timeout: int = 120) -> Object:
        response = object_value(json.loads(self.transport.execute('ha', *arguments, '--raw-json', timeout=timeout)))
        if response.get('result') != 'ok':
            raise ValueError(f'Home Assistant: {response.get("message", "request failed")}')
        data = response.get('data')
        return {} if data is None else object_value(data)

    def app(self, slug: str) -> AppInfo:
        info = AppInfo.parse(self.ha('apps', 'info', slug))
        if info.slug != slug:
            raise ValueError(f'Home Assistant returned {info.slug} when requesting {slug}')
        return info

    def refresh_store(self, version: str) -> None:
        self.step('Reloading the Home Assistant Store')
        try:
            self.ha('store', 'reload')
        except subprocess.CalledProcessError as error:
            if 'context deadline exceeded' not in (error.stderr or ''):
                raise
            self.output.console.print('Store reload timed out; checking development app discovery.')
        for attempt in range(3):
            info = self.app(DEV_SLUG)
            if info.available and info.latest == version:
                return
            if attempt < 2:
                time.sleep(2)
        raise ValueError(f'Store did not expose {DEV_SLUG} version {version}')

    def switch(self, stable: AppInfo) -> None:
        try:
            if stable.state == 'started':
                self.step(f'Stopping {stable.slug}')
                self.ha('apps', 'stop', stable.slug)
            self.step(f'Starting {DEV_SLUG}')
            self.ha('apps', 'start', DEV_SLUG)
            if self.app(DEV_SLUG).state != 'started':
                raise ValueError('The development app did not reach the started state')
        except OSError, ValueError, subprocess.SubprocessError:
            failed_stage = self.stage
            recovery = [('stop', DEV_SLUG)]
            if stable.state == 'started':
                recovery.append(('start', stable.slug))
            for action, slug in recovery:
                try:
                    self.ha('apps', action, slug)
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    self.output.console.print(f'Could not {action} {slug} during rollback: {error}')
            self.stage = failed_stage
            raise

    def install(self, archive: Path, apps_dir: str, stable_slug: str, version: str) -> None:
        temporary: str | None = None
        try:
            self.step(f'Checking {stable_slug} and the local apps directory')
            stable = self.app(stable_slug)
            if not stable.installed or stable.state not in ('started', 'stopped'):
                raise ValueError(f'Stable app {stable_slug} must be installed and started or stopped')
            self.transport.execute('test', '-d', apps_dir)
            destination = str(PurePosixPath(apps_dir) / 'garmin_toolkit-dev')
            self.transport.execute('test', '!', '-L', destination)
            self.step('Transferring the development bundle')
            temporary = self.transport.execute('mktemp', '/tmp/garmin-toolkit-dev.XXXXXX').strip()
            if not temporary.startswith('/tmp/garmin-toolkit-dev.') or any(c.isspace() for c in temporary):
                temporary = None
                raise ValueError('Unexpected remote temporary archive path')
            self.transport.upload(archive, temporary)
            self.step('Extracting the development bundle')
            self.transport.execute('mkdir', '-p', destination)
            self.transport.execute('tar', 'xzf', temporary, '-C', destination, timeout=300)
            self.refresh_store(version)
            dev = self.app(DEV_SLUG)
            if dev.installed:
                if dev.state == 'started':
                    self.step(f'Stopping {DEV_SLUG} before rebuilding')
                    self.ha('apps', 'stop', DEV_SLUG)
                self.step(f'Rebuilding {DEV_SLUG}')
                self.ha('apps', 'rebuild', DEV_SLUG, timeout=1800)
            else:
                self.step(f'Installing {DEV_SLUG}')
                self.ha('apps', 'install', DEV_SLUG, timeout=1800)
            self.switch(stable)
        except OSError, ValueError, subprocess.SubprocessError:
            self.output.panel(f'Failed while {self.stage}', title='Deployment step failed', style='red', error=True)
            raise
        finally:
            if temporary is not None:
                try:
                    self.transport.execute('rm', '-f', '--', temporary)
                except (OSError, subprocess.SubprocessError) as error:
                    self.output.console.print(f'Could not remove remote temporary bundle: {error}')
