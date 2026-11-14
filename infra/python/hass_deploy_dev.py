"""Build a separate development app and install it through the user's SSH configuration."""

import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

import tyro

from cli_output import TerminalOutput, child_environment
from hass_deploy_remote import Installer, SshTransport

ROOT = Path(__file__).resolve().parents[2]


@dataclass(frozen=True, kw_only=True)
class Options:
    """Build locally, transfer via SSH, and switch from stable to the separate dev app."""

    host: tyro.conf.Positional[str]
    arch: Literal['amd64', 'aarch64'] = 'aarch64'
    apps_dir: str = '/local_apps'
    stable_slug: str = 'af63a031_garmin_toolkit'
    build_only: bool = False


def deploy(options: Options) -> None:
    if not options.host or options.host.startswith('-') or any(char.isspace() for char in options.host):
        raise ValueError('Provide an SSH host or alias, not an SSH option')
    if not options.apps_dir.startswith('/') or '\n' in options.apps_dir:
        raise ValueError('The remote local-app directory must be an absolute path')
    if not options.stable_slug or any(
        char not in 'abcdefghijklmnopqrstuvwxyz0123456789_-' for char in options.stable_slug
    ):
        raise ValueError('Invalid stable app slug')
    output = TerminalOutput()
    environment = child_environment()
    attribute = f'git+file://{ROOT}#garmin-hass-dev-{options.arch}'
    output.panel('Building the development app bundle locally.', title='Garmin Toolkit Dev')
    result = subprocess.run(
        ['nix', 'build', attribute, '--no-link', '--print-out-paths', '--no-update-lock-file'],
        cwd=ROOT,
        env=environment,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    archive = Path(result.stdout.strip())
    if not archive.is_file():
        raise ValueError('Nix did not return one bundle file')
    output.panel(str(archive), title='Development bundle')
    if options.build_only:
        return
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())
    version = workspace['workspace']['package']['version'] + '-dev'
    Installer(SshTransport(options.host), output).install(archive, options.apps_dir, options.stable_slug, version)
    output.panel('Open Garmin Toolkit Dev in Home Assistant to verify the fixes.', title='Installed')


def main() -> None:
    output = TerminalOutput()
    output.install_tracebacks()
    try:
        deploy(tyro.cli(Options))
    except subprocess.CalledProcessError as error:
        if error.stderr:
            output.ansi(error.stderr, title='Remote command error', error=True)
        output.panel(str(error), title='Development deployment failed', style='red', error=True)
        sys.exit(error.returncode if error.returncode >= 0 else 128 - error.returncode)
    except subprocess.TimeoutExpired as error:
        output.panel(str(error), title='Development deployment timed out', style='red', error=True)
        sys.exit(124)
    except (OSError, ValueError) as error:
        output.panel(str(error), title='Development deployment failed', style='red', error=True)
        sys.exit(1)


if __name__ == '__main__':
    main()
