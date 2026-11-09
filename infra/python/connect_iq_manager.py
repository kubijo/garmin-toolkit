"""Owner-downloaded SDK Manager, isolated from Nix inputs and container images."""

import hashlib
import os
import platform
import shutil
import stat
import subprocess
import tempfile
import uuid
import zipfile
from collections.abc import Generator, Mapping
from contextlib import contextmanager
from pathlib import Path, PurePosixPath

from cli_output import TerminalOutput
from processes import cleanup_signals

OUTPUT = TerminalOutput()


def extract_manager(archive: Path, state: Path) -> Path:
    with archive.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    destination = state / 'manager' / digest
    with zipfile.ZipFile(archive) as package:
        entries = package.infolist()
        names = [PurePosixPath(entry.filename) for entry in entries]
        if (
            len(entries) > 4096
            or sum(entry.file_size for entry in entries) > 512 * 1024 * 1024
            or len(set(names)) != len(names)
            or any(
                name.is_absolute()
                or '..' in name.parts
                or '\\' in entry.filename
                or stat.S_ISLNK(entry.external_attr >> 16)
                for name, entry in zip(names, entries, strict=True)
            )
            or PurePosixPath('bin/sdkmanager') not in names
        ):
            raise ValueError('Expected a bounded Linux SDK Manager ZIP without links or unsafe paths')
        reused = destination.exists()
        if destination.is_symlink():
            raise ValueError('SDK Manager cache must not be a symbolic link')
        destination.mkdir(parents=True, exist_ok=True, mode=0o700)
        for name, entry in zip(names, entries, strict=True):
            target = destination.joinpath(*name.parts)
            if target.is_symlink() or not target.resolve().is_relative_to(destination):
                raise ValueError('SDK Manager cache contains an unsafe path')
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True, mode=0o700)
                continue
            if reused:
                if not target.is_file() or target.read_bytes() != package.read(entry):
                    raise ValueError('SDK Manager cache changed; remove this cache directory before retrying')
            else:
                target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                with package.open(entry) as source, target.open('xb') as output:
                    shutil.copyfileobj(source, output)
                target.chmod(0o700 if entry.external_attr >> 16 & 0o111 else 0o600)
    manager = destination / 'bin/sdkmanager'
    manager.chmod(0o700)
    return manager


class Runtime:
    def __init__(self, root: Path, state: Path, *, certificate: Path | None = None) -> None:
        if platform.system() != 'Linux' or platform.machine() not in ('x86_64', 'amd64'):
            raise ValueError('The Connect IQ GUI runtime requires an x86_64 Linux host')
        self.context = root / 'infra/connect-iq'
        self.state = state
        selected = os.environ.get('GARMIN_TOOLKIT_CIQ_CA_BUNDLE')
        if certificate is None and selected is None:
            raise ValueError('Use the Nix Connect IQ entrypoint to provide its pinned CA bundle')
        self.certificate = certificate or Path(str(selected))
        self.podman = [
            'podman',
            '--root',
            str(state / 'containers'),
            '--runroot',
            str(state / 'run'),
            '--storage-driver',
            'vfs',
        ]
        digest = hashlib.sha256()
        for name in ('Dockerfile', 'sources.list'):
            digest.update((self.context / name).read_bytes())
        digest.update(self.certificate.read_bytes())
        self.image = 'localhost/garmin-connect-iq-manager:' + digest.hexdigest()

    def ensure_image(self) -> None:
        self.state.mkdir(parents=True, exist_ok=True, mode=0o700)
        cached = subprocess.run(
            [*self.podman, 'image', 'exists', self.image], check=False, capture_output=True, text=True, timeout=30
        )
        if cached.returncode == 0:
            return
        if cached.returncode != 1:
            raise subprocess.CalledProcessError(cached.returncode, cached.args, stderr=cached.stderr)
        OUTPUT.panel(str(self.state / 'runtime-build.log'), title='Preparing Connect IQ compatibility runtime')
        with tempfile.TemporaryDirectory(dir=self.state, prefix='runtime-context-') as temporary:
            context = Path(temporary)
            for name in ('Dockerfile', 'sources.list'):
                shutil.copyfile(self.context / name, context / name)
            shutil.copyfile(self.certificate, context / 'ca-bundle.crt')
            command = [
                *self.podman,
                'build',
                '--cpu-period=100000',
                '--cpu-quota=200000',
                '--memory=6g',
                '--tag',
                self.image,
                str(context),
            ]
            try:
                with (
                    (self.state / 'runtime-build.log').open('w') as log,
                    OUTPUT.progress('Preparing compatibility libraries'),
                ):
                    subprocess.run(
                        command, check=True, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, timeout=900
                    )
            except subprocess.CalledProcessError, subprocess.TimeoutExpired:
                OUTPUT.ansi(
                    (self.state / 'runtime-build.log').read_text(errors='replace'),
                    title='Compatibility runtime build failed',
                    error=True,
                )
                raise

    def command(self, manager: Path, *, gui: bool, environment: Mapping[str, str]) -> list[str]:
        home = Path.home()
        data = home / '.Garmin/ConnectIQ'
        manager_root = manager.parent.parent
        command = [
            *self.podman,
            'run',
            '--rm',
            '--cpus=2',
            '--memory=6g',
            '--network=host',
            '--userns=keep-id',
            '--user',
            f'{os.getuid()}:{os.getgid()}',
            '--cap-drop=ALL',
            '--security-opt=no-new-privileges',
            '--volume',
            f'{manager_root}:{manager_root}:ro',
        ]
        if not gui:
            return [*command, '--entrypoint', '/usr/bin/ldd', self.image, str(manager)]
        if not environment.get('DISPLAY'):
            raise ValueError('Connect IQ GUI tools need a desktop DISPLAY/XWayland session')
        data.mkdir(parents=True, exist_ok=True, mode=0o700)
        command.extend(
            [
                '--volume',
                f'{data}:{data}',
                '--volume',
                '/tmp/.X11-unix:/tmp/.X11-unix:ro',
                '--env',
                'HOME=' + str(home),
                '--env',
                'DISPLAY=' + environment['DISPLAY'],
                '--env',
                'GDK_BACKEND=x11',
                '--env',
                'LIBGL_ALWAYS_SOFTWARE=1',
                '--env',
                'XDG_CACHE_HOME=/tmp/cache',
                '--env',
                'XDG_CONFIG_HOME=/tmp/config',
            ]
        )
        authority = Path(environment.get('XAUTHORITY', str(home / '.Xauthority')))
        if authority.is_file():
            command.extend(['--volume', f'{authority}:{authority}:ro', '--env', 'XAUTHORITY=' + str(authority)])
        bus = environment.get('DBUS_SESSION_BUS_ADDRESS', '')
        if bus.startswith('unix:path='):
            socket = Path(bus.removeprefix('unix:path=').split(',')[0])
            if socket.is_socket():
                command.extend(['--volume', f'{socket}:{socket}:ro', '--env', 'DBUS_SESSION_BUS_ADDRESS=' + bus])
        return [*command, self.image, str(manager)]

    def verify(self, executable: Path) -> None:
        with self.container(executable, gui=False, environment={}) as command:
            result = subprocess.run(command, check=False, capture_output=True, text=True, timeout=60)
        log = self.state / (executable.name + '-libraries.log')
        log.write_text(result.stdout + result.stderr)
        if result.returncode or 'not found' in result.stdout or 'not found' in result.stderr:
            raise ValueError(executable.name + ' has unresolved libraries; inspect ' + str(log))

    @contextmanager
    def container(self, manager: Path, *, gui: bool, environment: Mapping[str, str]) -> Generator[list[str]]:
        name = 'garmin-ciq-manager-' + uuid.uuid4().hex
        command = self.command(manager, gui=gui, environment=environment)
        command[command.index('run') + 1 : command.index('run') + 1] = ['--name', name]
        try:
            yield command
        finally:
            with cleanup_signals():
                subprocess.run(
                    [*self.podman, 'rm', '--force', '--ignore', name],
                    check=True,
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    timeout=30,
                )

    def launch(self, manager: Path) -> None:
        with self.container(manager, gui=True, environment=os.environ) as command:
            subprocess.run(command, check=True, timeout=3600)
