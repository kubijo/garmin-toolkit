"""Explicit local Connect IQ builds and offline capture decoding; no device access."""

import json
import os
import shutil
import stat
import subprocess
from dataclasses import asdict
from pathlib import Path
from typing import Literal

import tyro.extras

from cli_output import TerminalOutput, child_environment
from connect_iq_protocol import decode

ROOT = Path(__file__).resolve().parents[2]
PROJECT = ROOT / 'apps/garmin-connect-iq'
OUTPUT = TerminalOutput()
Device = Literal['fenix8solar47mm', 'edge1050']


def local_material(path: Path, root: Path = ROOT) -> Path:
    resolved = path.resolve()
    if resolved.is_relative_to(root) and not resolved.is_relative_to(root / '.tmp'):
        raise ValueError('SDK and signing material must be outside the checkout or under ignored .tmp')
    return resolved


def private_key(path: Path, root: Path = ROOT) -> Path:
    resolved = local_material(path, root)
    metadata = resolved.stat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077 or metadata.st_uid != os.getuid():
        raise ValueError('Signing key must be an owner-only regular file owned by the current user')
    return resolved


def capture(*arguments: str) -> str:
    return subprocess.run(
        arguments, check=True, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30
    ).stdout


def bootstrap() -> None:
    """Show the owner-operated SDK setup; never download tools or accept agreements."""
    OUTPUT.panel(
        'Download SDK Manager yourself and put its Linux ZIP or macOS DMG under repository .tmp.\n'
        'https://developer.garmin.com/connect-iq/sdk/\n'
        'Run just connect-iq::setup. In Manager, install and activate an SDK plus fenix8solar47mm, then close it.\n'
        'Setup handles the signing key, prerequisite checks, and build. No radio access is performed.',
        title='Connect IQ local setup',
    )


def keygen(*, key: Path, from_key: Path | None = None) -> None:
    """Create an RSA-4096 PKCS#8 DER key, or re-encode an existing DER key into a new file."""
    path = local_material(key)
    if path.exists():
        if from_key is not None:
            raise ValueError('Key conversion requires an unused destination; existing keys are preserved')
        validate_key(private_key(path))
        return
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, 'wb') as destination:
            if from_key is not None:
                subprocess.run(
                    [
                        'openssl',
                        'pkcs8',
                        '-topk8',
                        '-nocrypt',
                        '-inform',
                        'DER',
                        '-outform',
                        'DER',
                        '-in',
                        str(private_key(from_key)),
                    ],
                    check=True,
                    stdin=subprocess.DEVNULL,
                    stdout=destination,
                    timeout=30,
                )
            else:
                generated = subprocess.run(
                    ['openssl', 'genpkey', '-algorithm', 'RSA', '-pkeyopt', 'rsa_keygen_bits:4096'],
                    check=True,
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    timeout=120,
                )
                subprocess.run(
                    ['openssl', 'pkcs8', '-topk8', '-nocrypt', '-outform', 'DER'],
                    check=True,
                    input=generated.stdout,
                    stdout=destination,
                    capture_output=False,
                    timeout=30,
                )
        validate_key(path)
    except BaseException:
        path.unlink()
        raise


def validate_key(path: Path) -> None:
    capture('openssl', 'pkcs8', '-inform', 'DER', '-in', str(path), '-nocrypt', '-out', os.devnull)
    capture('openssl', 'rsa', '-inform', 'DER', '-in', str(path), '-check', '-noout')
    description = capture('openssl', 'pkey', '-inform', 'DER', '-in', str(path), '-text_pub', '-noout')
    if not description.startswith('Public-Key: (4096 bit)'):
        raise ValueError('Expected an RSA-4096 DER signing key')


def doctor(*, sdk: Path, key: Path) -> None:
    """Check local compiler, Java, OpenSSL, key format and key custody."""
    compiler = local_material(sdk) / 'bin/monkeyc'
    if not compiler.is_file() or not os.access(compiler, os.X_OK):
        raise ValueError('SDK must contain an executable bin/monkeyc')
    if shutil.which('java') is None or shutil.which('openssl') is None:
        raise ValueError('Java and OpenSSL must be on PATH')
    validate_key(private_key(key))
    OUTPUT.ansi(capture(str(compiler), '-v'), title='Local Connect IQ compiler')


def build(*, sdk: Path, key: Path, output: Path, device: Device = 'fenix8solar47mm') -> None:
    """Compile the foreground probe locally; Garmin tools and outputs stay outside CI/releases."""
    directory = output.resolve()
    if not directory.is_relative_to(ROOT / '.tmp') or directory == ROOT / '.tmp':
        raise ValueError('Build output must be a new directory below repository .tmp')
    doctor(sdk=sdk, key=key)
    directory.mkdir(parents=True, exist_ok=False)
    log = directory / 'build.log'
    OUTPUT.panel(str(log), title='Retained compiler log')
    try:
        with log.open('w') as destination, OUTPUT.progress('Compiling the foreground probe'):
            subprocess.run(
                [
                    str(local_material(sdk) / 'bin/monkeyc'),
                    '-f',
                    str(PROJECT / 'monkey.jungle'),
                    '-d',
                    device,
                    '-y',
                    str(private_key(key)),
                    '-o',
                    str(directory / 'probe.prg'),
                    '-w',
                ],
                cwd=PROJECT,
                env=child_environment(),
                stdin=subprocess.DEVNULL,
                stdout=destination,
                stderr=subprocess.STDOUT,
                check=True,
                timeout=300,
            )
    finally:
        transcript = log.read_text(errors='replace')
        if transcript:
            OUTPUT.ansi(transcript, title='Compiler output')
    binary = directory / 'probe.prg'
    if not binary.is_file():
        raise ValueError('Compiler reported success without producing probe.prg')
    artifacts = ROOT / 'dist/connect-iq'
    artifact = artifacts / 'edge1050/probe.prg' if device == 'edge1050' else artifacts / 'probe.prg'
    artifact.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(binary, artifact)
    OUTPUT.panel(str(artifact), title='Probe build ready')


def inspect(*, response: Path, sequence: int) -> None:
    """Decode one captured binary response without contacting Bluetooth or HASS."""
    with response.open('rb') as source:
        payload = source.read(21)
    snapshot = decode(payload, sequence)
    OUTPUT.panel(json.dumps(asdict(snapshot), indent=2), title='Captured telemetry')


def main() -> None:
    from connect_iq_host import host
    from connect_iq_setup import prepare, rebuild, setup
    from connect_iq_simulator import preview, test

    OUTPUT.install_tracebacks()
    try:
        tyro.extras.subcommand_cli_from_dict(
            {
                'setup': setup,
                'prepare': prepare,
                'rebuild': rebuild,
                'bootstrap': bootstrap,
                'keygen': keygen,
                'doctor': doctor,
                'build': build,
                'inspect': inspect,
                'host': host,
                'test': test,
                'preview': preview,
            }
        )
    except KeyboardInterrupt:
        raise SystemExit(130) from None
    except subprocess.TimeoutExpired:
        OUTPUT.panel('Command timed out.', title='Connect IQ incomplete', error=True)
        raise SystemExit(124) from None
    except subprocess.CalledProcessError as error:
        if error.stderr:
            message = error.stderr.decode(errors='replace') if isinstance(error.stderr, bytes) else str(error.stderr)
            OUTPUT.ansi(message, title='Local tool output', error=True)
        OUTPUT.panel('Local tool failed; inspect its output.', title='Connect IQ incomplete', error=True)
        raise SystemExit(error.returncode if error.returncode >= 0 else 128 - error.returncode) from None
    except FileNotFoundError as error:
        OUTPUT.panel(str(error), title='Connect IQ incomplete', error=True)
        raise SystemExit(127) from None
    except PermissionError as error:
        OUTPUT.panel(str(error), title='Connect IQ incomplete', error=True)
        raise SystemExit(126) from None
    except (OSError, ValueError) as error:
        OUTPUT.panel(str(error), title='Connect IQ incomplete', error=True)
        raise SystemExit(1) from None


if __name__ == '__main__':
    main()
