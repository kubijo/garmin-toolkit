"""Own a bounded, interactive HAOS app session and its disposable browser."""

import json
import os
import re
import secrets
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from contextlib import nullcontext
from dataclasses import dataclass, replace
from http.client import HTTPMessage
from pathlib import Path
from typing import IO, Annotated, cast

import tyro
from rich.text import Text

import haos_app_vm as vm
import haos_workflow
from haos_lifecycle import Lifecycle
from haos_platform import acceleration, baseline_fixture
from haos_upgrade import published_baseline
from hass_release_build import verify_build_inputs
from processes import ACCEPTANCE_SCRATCH, session_scratch, stop_group

ROOT = Path(__file__).resolve().parents[2]
OVERLAY_LIMIT = 16 * 1024**3


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Keep guest requests away from unrelated host listeners."""

    def redirect_request(
        self, req: urllib.request.Request, fp: IO[bytes], code: int, msg: str, headers: HTTPMessage, newurl: str
    ) -> None:
        return None


HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


@dataclass(frozen=True)
class Run(vm.Options):
    """Install the app and verify ingress in isolated headless Chrome."""

    timeout: int = 1800
    """Maximum startup time in seconds."""

    session_timeout: int = 7200
    """Inspection time after the app is ready, in seconds."""

    port: int = 0
    """Loopback Home Assistant port; zero chooses an available port."""

    verify_browser: bool = True
    """Verify authenticated ingress in an owned headless Chrome session."""


@dataclass(frozen=True)
class Stop:
    """Stop a session and wait for its owned VM and scratch files to disappear."""

    session_id: str


@dataclass(frozen=True)
class Check(Run):
    """Verify populated ingress, app/HAOS restarts, and cold backup/restore, then clean up."""

    timeout: int = 1800
    """Maximum startup and workflow time in seconds."""

    session_timeout: tyro.conf.Suppress[int] = 7200
    verify_browser: tyro.conf.Suppress[bool] = True

    build_receipt: Path | None = None
    """Verify the downloaded native export and SBOM against its receipt and committed Git source."""

    evidence_directory: Path | None = None
    """Fresh evidence directory under repository .tmp; defaults to an owned session directory."""

    lifecycle: bool = False
    """Test upgrade, backup recovery, and uninstall/reinstall; requires repository and receipt."""

    baseline_release: Path | None = None
    """Published release directory with image, release.json, SHA256SUMS, and their attestation bundles."""

    candidate_version: str | None = None
    """Private Supervisor candidate version for lifecycle checks; leaves OCI archive bytes unchanged."""

    install_links: bool = False
    """Verify README/quickstart My Home Assistant links against the public repository in this disposable browser."""


@dataclass(frozen=True, kw_only=True)
class Options:
    """Manage a disposable Home Assistant app session."""

    command: Annotated[
        (
            Annotated[Run, tyro.conf.subcommand('run', prefix_name=False)]
            | Annotated[Check, tyro.conf.subcommand('check', prefix_name=False)]
            | Annotated[Stop, tyro.conf.subcommand('stop', prefix_name=False)]
        ),
        tyro.conf.arg(name=''),
    ]


class SessionStopped(Exception):
    """An explicit stop interrupted the session."""


def process_identity(pid: int) -> str:
    return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]


def save_state(path: Path, state: dict[str, object]) -> None:
    path.write_text(json.dumps(state, indent=2) + '\n')
    path.chmod(0o600)


def request_json(
    base: str,
    path: str,
    payload: dict[str, object] | None = None,
    *,
    token: str | None = None,
    form: bool = False,
) -> object:
    headers = {'Content-Type': 'application/x-www-form-urlencoded' if form else 'application/json'}
    if token is not None:
        headers['Authorization'] = f'Bearer {token}'
    body = (
        None
        if payload is None
        else (urllib.parse.urlencode(payload).encode() if form else json.dumps(payload).encode())
    )
    request = urllib.request.Request(base + path, data=body, headers=headers)
    try:
        with HTTP.open(request, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        with error:
            details = error.read(1000).decode(errors='replace')
        raise RuntimeError(f'Home Assistant {path} returned HTTP {error.code}: {details}') from error


def object_response(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise RuntimeError('Home Assistant returned an unexpected JSON response')
    return cast('dict[str, object]', value)


def wait_for_core(
    process: subprocess.Popen[str], tunnel: subprocess.Popen[str], base: str, deadline: float, *, onboarding: bool
) -> None:
    notice = 0.0
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('HAOS exited while waiting for Home Assistant Core')
        if tunnel.poll() is not None:
            raise RuntimeError('The owned Home Assistant HTTP tunnel exited before Core was ready')
        try:
            ready = request_json(base, '/api/onboarding' if onboarding else '/auth/providers')
            if not onboarding:
                ready = object_response(ready).get('providers')
            if isinstance(ready, list):
                return
        except OSError, ValueError, RuntimeError:
            pass
        if time.monotonic() >= notice:
            print('Waiting for full Home Assistant Core authentication...', flush=True)
            notice = time.monotonic() + 30
        time.sleep(3)
    raise TimeoutError('Home Assistant Core did not become ready within the startup window')


def onboard(base: str, password: str) -> dict[str, object]:
    client_id = base + '/'
    user = object_response(
        request_json(
            base,
            '/api/onboarding/users',
            {
                'name': 'Disposable HAOS',
                'username': 'admin',
                'password': password,
                'client_id': client_id,
                'language': 'en',
            },
        )
    )
    tokens = object_response(
        request_json(
            base,
            '/auth/token',
            {
                'grant_type': 'authorization_code',
                'code': user['auth_code'],
                'client_id': client_id,
            },
            form=True,
        )
    )
    access = str(tokens['access_token'])
    request_json(base, '/api/onboarding/core_config', {}, token=access)
    request_json(base, '/api/onboarding/analytics', {}, token=access)
    request_json(
        base,
        '/api/onboarding/integration',
        {
            'client_id': client_id,
            'redirect_uri': client_id + '?auth_callback=1',
        },
        token=access,
    )
    tokens.update(
        hassUrl=base, clientId=client_id, expires=int(time.time() * 1000) + int(str(tokens['expires_in'])) * 1000
    )
    return tokens


def launch_browser(state_path: Path, evidence: Path) -> subprocess.Popen[str]:
    with (evidence / 'browser.log').open('w') as log:
        return subprocess.Popen(
            [
                'node',
                str(ROOT / 'infra/javascript/haos-browser.mts'),
                str(state_path),
                str(evidence),
            ],
            stdout=log,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
        )


def launch_tunnel(
    process: subprocess.Popen[str], key: Path, ssh_port: int, http_port: int, evidence: Path, deadline: float
) -> subprocess.Popen[str]:
    last_error: Exception | None = None
    while (remaining := deadline - time.monotonic()) > 0:
        if process.poll() is not None:
            raise RuntimeError('HAOS exited while waiting for Home Assistant Core installation')
        try:
            core = vm.guest_data(key, ssh_port, 'ha core info', timeout=max(1, int(min(30, remaining))))
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            last_error = error
        else:
            last_error = None
            if core.get('version') != 'landingpage':
                core_port = int(str(core['port']))
                if core.get('ssl') is not False or not 1 <= core_port <= 65535:
                    raise RuntimeError('The disposable Core must expose a valid plain HTTP port')
                return vm.forward_port(key, ssh_port, http_port, core_port, evidence / 'tunnel.log')
        print('Waiting for Home Assistant Core installation...', flush=True)
        time.sleep(min(5, max(0.0, deadline - time.monotonic())))
    raise TimeoutError('Home Assistant Core did not become available within the startup window') from last_error


def wait_browser(
    browser: subprocess.Popen[str], evidence: Path, checkpoint: str = 'ready.json', *, timeout: int = 120
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if (evidence / checkpoint).is_file():
            return
        if browser.poll() is not None:
            raise RuntimeError(f'Browser verification failed: {(evidence / "browser.log").read_text()[-3000:]}')
        time.sleep(0.5)
    raise TimeoutError(f'Browser did not reach {checkpoint} in {timeout} seconds')


def run_session(options: Run) -> int:
    if (
        isinstance(options, Check)
        and not options.lifecycle
        and (options.baseline_release is not None or options.candidate_version is not None)
    ):
        raise ValueError('Published baseline and private candidate versions require lifecycle acceptance')
    if (
        isinstance(options, Check)
        and options.install_links
        and options.repository_url != 'https://github.com/kubijo/garmin-toolkit#hass-apps'
    ):
        raise ValueError('Installation link acceptance requires the public hass-apps repository URL')
    if (
        isinstance(options, Check)
        and options.lifecycle
        and (options.repository is None or options.build_receipt is None)
    ):
        raise ValueError('Lifecycle acceptance requires a generated repository and native build receipt')
    if not 10 <= options.timeout <= 1800 or not 1 <= options.session_timeout <= 7200 or not 0 <= options.port <= 65535:
        raise ValueError('Startup must be 10-1800s, inspection 1-7200s, and port 0-65535')
    if isinstance(options, Check) and not options.verify_browser:
        raise ValueError('The populated workflow requires browser verification')
    guest = vm.validate_options(options)
    build = (
        verify_build_inputs(options.app_archive, options.build_receipt, ROOT)
        if isinstance(options, Check) and options.build_receipt is not None
        else None
    )
    previous = (
        published_baseline(options.baseline_release, ROOT, guest)
        if isinstance(options, Check) and options.baseline_release is not None
        else None
    )
    if previous is not None:
        baseline_version = previous[1]['version']
        if baseline_version == (options.candidate_version or vm.test_config()[1]):
            raise ValueError('Published baseline and candidate require distinct Supervisor versions')
    vm.SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
    scratch = session_scratch(vm.SCRATCH_ROOT)
    session_id = secrets.token_hex(6)
    evidence = vm.SCRATCH_ROOT / 'haos-evidence' / session_id
    if isinstance(options, Check) and options.evidence_directory is not None:
        evidence = options.evidence_directory.resolve()
        if not evidence.is_relative_to(vm.SCRATCH_ROOT.resolve()):
            raise ValueError('Acceptance evidence must remain under repository .tmp')
    if ACCEPTANCE_SCRATCH in os.environ and evidence.resolve().is_relative_to(scratch.resolve()):
        raise ValueError('Acceptance evidence must remain outside disposable scratch')
    evidence.mkdir(parents=True)
    if build is not None:
        (evidence / 'native-build.json').write_text(json.dumps(build, indent=2) + '\n')
    deadline = time.monotonic() + options.timeout
    browser: subprocess.Popen[str] | None = None
    tunnel: subprocess.Popen[str] | None = None
    with (
        tempfile.TemporaryDirectory(prefix=f'haos-session.{session_id}.', dir=scratch) as directory,
        (
            nullcontext(str(scratch))
            if ACCEPTANCE_SCRATCH in os.environ
            else tempfile.TemporaryDirectory(prefix='c.', dir=vm.SCRATCH_ROOT)
        ) as browser_directory,
    ):
        runtime = Path(directory)
        runtime.chmod(0o700)
        if isinstance(options, Check) and options.lifecycle:
            if previous is None:
                baseline = runtime / 'baseline-fixture.oci.tar'
                baseline_fixture(options.app_archive, baseline)
            else:
                baseline, identity = previous
                encoded = json.dumps(identity, indent=2) + '\n'
                (runtime / 'published-baseline.json').write_text(encoded)
                (evidence / 'published-baseline.json').write_text(encoded)
            options = replace(options, baseline_archive=baseline)
        state_path = runtime / 'session.json'
        http_port = options.port or vm.free_port()
        base = f'http://127.0.0.1:{http_port}'
        state: dict[str, object] = {
            'id': session_id,
            'pid': os.getpid(),
            'process_identity': process_identity(os.getpid()),
            'phase': 'starting',
            'base_url': base,
            'architecture': guest.architecture,
            'acceleration': acceleration(guest),
            'password': 'admin',
            'browser_temporary': browser_directory,
            'workflow': isinstance(options, Check),
            'install_links': isinstance(options, Check) and options.install_links,
            'repository_url': options.repository_url,
        }
        save_state(state_path, state)
        print(f'Session {session_id}: {guest.architecture}, {state["acceleration"]}; runtime {runtime}', flush=True)
        print(f'Stop with: just hass::haos-app-stop {session_id}', flush=True)

        def booted(unit: str, ssh_port: int) -> None:
            state.update(unit=unit, ssh_port=ssh_port)
            save_state(state_path, state)

        def started(process: subprocess.Popen[str], key: Path, ssh_port: int, app_slug: str) -> None:
            nonlocal browser, tunnel
            tunnel = launch_tunnel(process, key, ssh_port, http_port, evidence, deadline)
            wait_for_core(process, tunnel, base, deadline, onboarding=True)
            tokens = onboard(base, str(state['password']))
            state['tokens'] = tokens
            state['url'] = base + '/app/' + app_slug
            state['app_slug'] = app_slug
            state['phase'] = 'verifying'
            save_state(state_path, state)
            versions = vm.guest_data(key, ssh_port, 'ha info')
            save_state(evidence / 'versions.json', versions)
            (evidence / 'installation.json').write_bytes((runtime / 'installation.json').read_bytes())
            if options.verify_browser:
                assert browser is not None
                wait_browser(browser, evidence)
                if isinstance(options, Check):
                    if options.install_links:
                        # The link must offer a first install, rather than find an existing repository.
                        vm.run(vm.ssh_command(key, ssh_port, f'ha apps uninstall {app_slug}'), timeout=180)
                        repository = shlex.quote(app_slug.partition('_')[0])
                        vm.run(
                            vm.ssh_command(key, ssh_port, 'ha store remove ' + repository),
                            timeout=90,
                        )
                        (evidence / 'install-links-prepared.json').write_text('{}\n')
                        wait_browser(browser, evidence, 'install-links.json', timeout=600)

                    def reboot() -> dict[str, str]:
                        nonlocal tunnel
                        assert tunnel is not None
                        stop_group(tunnel)
                        boot_ids = vm.reboot_guest(
                            process, runtime, key, ssh_port, timeout=300, overlay_limit=OVERLAY_LIMIT
                        )
                        tunnel = launch_tunnel(process, key, ssh_port, http_port, evidence, deadline)
                        wait_for_core(process, tunnel, base, deadline, onboarding=False)
                        vm.assert_started(key, ssh_port, app_slug)
                        installed = object_response(json.loads((evidence / 'installation.json').read_bytes()))
                        vm.assert_installed_image(
                            key,
                            ssh_port,
                            app_slug,
                            reference=str(installed['image_reference']),
                            digest=str(installed['registry_digest']),
                            config_digest=str(installed['config_digest']),
                            architecture=str(installed['architecture']),
                        )
                        return boot_ids

                    lifecycle = (
                        Lifecycle(runtime, evidence, key, ssh_port, app_slug, process, OVERLAY_LIMIT)
                        if options.lifecycle
                        else None
                    )
                    haos_workflow.exercise(
                        browser, runtime, evidence, key, ssh_port, app_slug, reboot=reboot, lifecycle=lifecycle
                    )
                    return
            state.update(phase='ready', expires_at=time.time() + options.session_timeout)
            save_state(state_path, state)
            signal.setitimer(signal.ITIMER_REAL, 0)
            print(f'Ready: {state["url"]}\nDisposable login: admin / {state["password"]}', flush=True)
            print(f'Session expires after {options.session_timeout}s; screenshot: {evidence / "ready.png"}', flush=True)
            until = time.monotonic() + options.session_timeout
            while time.monotonic() < until:
                if process.poll() is not None:
                    raise RuntimeError('The interactive HAOS VM exited unexpectedly')
                if tunnel.poll() is not None:
                    raise RuntimeError('The owned Home Assistant HTTP tunnel exited unexpectedly')
                time.sleep(1)
            print('Inspection timeout reached; stopping the owned HAOS session.', flush=True)

        try:
            if options.verify_browser:
                browser = launch_browser(state_path, evidence)
                wait_browser(browser, evidence, 'browser-started.json')
            image = vm.prepare_runtime(options, runtime, guest)
            vm.boot(
                options,
                runtime,
                image,
                on_started=started,
                on_boot=booted,
                lifetime=options.timeout + (0 if isinstance(options, Check) else options.session_timeout) + 30,
                overlay_limit=OVERLAY_LIMIT,
            )
        except BaseException:
            serial = runtime / 'serial.log'
            if serial.is_file():
                (evidence / 'serial-tail.log').write_text(serial.read_text(errors='replace')[-64_000:])
            raise
        finally:
            try:
                if browser is not None:
                    stop_group(browser)
            finally:
                if tunnel is not None:
                    stop_group(tunnel)
    return 0


def stop_session(session_id: str) -> int:
    if not re.fullmatch('[a-f0-9]{12}', session_id):
        raise ValueError('Invalid HAOS session ID')
    directories = [
        *vm.SCRATCH_ROOT.glob(f'haos-session.{session_id}.*'),
        *vm.SCRATCH_ROOT.glob(f'a.*/haos-session.{session_id}.*'),
    ]
    if len(directories) != 1:
        raise ValueError(f'No active HAOS session {session_id}')
    runtime = directories[0]
    state = object_response(json.loads((runtime / 'session.json').read_text()))
    pid = int(str(state['pid']))
    if process_identity(pid) != state['process_identity']:
        raise RuntimeError('The session runner identity no longer matches; refusing to signal another process')
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 35
    while runtime.exists() and time.monotonic() < deadline:
        time.sleep(0.2)
    if runtime.exists():
        raise TimeoutError('Session runner did not remove its scratch directory after stop')
    address = urllib.parse.urlparse(str(state['base_url']))
    with socket.socket() as probe:
        probe.settimeout(1)
        if probe.connect_ex(('127.0.0.1', address.port or 8123)) == 0:
            raise RuntimeError('The stopped session HTTP listener is still open')
    print(f'Session {session_id} stopped; VM, browser, listener, and scratch removed.', flush=True)
    return 0


def interrupted(_signal: int, _frame: object) -> None:
    raise SessionStopped


def startup_expired(_signal: int, _frame: object) -> None:
    raise TimeoutError('The HAOS session exceeded its startup time limit')


def main(command: Run | Stop) -> int:
    previous = signal.signal(signal.SIGTERM, interrupted) if isinstance(command, Run) else None
    previous_alarm = signal.signal(signal.SIGALRM, startup_expired) if isinstance(command, Run) else None
    try:
        if isinstance(command, Run):
            signal.setitimer(signal.ITIMER_REAL, command.timeout)
        return run_session(command) if isinstance(command, Run) else stop_session(command.session_id)
    except SessionStopped:
        print('HAOS session stopped and cleaned up.', flush=True)
        return 143 if isinstance(command, Check) else 0
    except KeyboardInterrupt:
        return 130
    except (subprocess.TimeoutExpired, TimeoutError) as error:
        vm.OUTPUT.panel(Text(str(error)), title='HAOS session timed out', error=True)
        return 124
    except (OSError, ValueError, RuntimeError) as error:
        vm.OUTPUT.panel(Text(str(error)), title='HAOS session failed', error=True)
        return 1
    finally:
        if previous is not None:
            signal.signal(signal.SIGTERM, previous)
        if previous_alarm is not None:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, previous_alarm)


if __name__ == '__main__':
    vm.OUTPUT.install_tracebacks()
    sys.exit(main(tyro.cli(Options).command))
