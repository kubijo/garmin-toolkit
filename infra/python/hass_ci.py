"""Executable CI phases; Actions owns permissions, dependencies, signing, and artifact transport."""

import os
import re
import shutil
import signal
import subprocess
import sys
from collections.abc import Generator
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path
from time import monotonic
from typing import Annotated, Literal

import tyro

import hass_preflight
from hass_publish import (
    IMAGE,
    REMOTE,
    CleanupStaging,
    CompletePublication,
    PreparePublication,
    PromoteDraft,
    ReleaseEvidence,
    ReleaseGuard,
    RollbackStore,
    StagePublication,
    cleanup_staging,
    complete_publication,
    package_evidence,
    prepare_publication,
    promote_draft,
    qualify_promotion,
    release_guard,
    rollback_store,
    stage_image,
    stage_publication,
)
from hass_release_build import NativeBuild, record_build
from hass_sources import BundleSources, bundle_sources
from json_data import object_rows, object_value, read_json, text
from processes import run_owned, terminated

NIX_LIMITS = ('--max-jobs', '1', '--cores', '2', '--no-update-lock-file')
PUBLICATION = Path('.tmp/publication')
NATIVE = Path('.tmp/native')
SOURCES = Path('.tmp/sources')
BUNDLES = Path('.tmp/bundles')


@dataclass(frozen=True, kw_only=True)
class Options:
    command: Annotated[
        Literal['native', 'draft', 'preflight', 'complete', 'verify-preflight', 'promote', 'cleanup'],
        tyro.conf.Positional,
    ]
    system: Literal['x86_64-linux', 'aarch64-linux'] = 'x86_64-linux'
    release: Literal['true', 'false'] = 'false'
    sources: Literal['true', 'false'] = 'false'


def progress(message: str) -> None:
    with suppress(OSError, ValueError):
        print(message, flush=True)


@contextmanager
def phase(name: str) -> Generator[None]:
    actions = os.environ.get('GITHUB_ACTIONS') == 'true'
    started, status = monotonic(), 'failed'
    progress(f'::group::{name}' if actions else f'=== {name}')
    try:
        yield
        status = 'passed'
    finally:
        if actions:
            progress('::endgroup::')
        progress(f'{name}: {status} ({monotonic() - started:.1f}s)')


def run(*arguments: str, capture: bool = False, secret: str | None = None) -> str:
    result = subprocess.run(
        arguments, check=True, text=True, input=secret, stdout=subprocess.PIPE if capture else None, timeout=7200
    )
    return result.stdout.strip() if capture else ''


def build(attribute: str) -> Path:
    return Path(run('nix', 'build', '-L', attribute, '--no-link', '--print-out-paths', *NIX_LIMITS, capture=True))


def outputs(**values: str) -> None:
    if any(not re.fullmatch(r'[a-z_]+', key) or '\n' in value or '\r' in value for key, value in values.items()):
        raise ValueError('Invalid single-line GitHub output')
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as destination:
        destination.writelines(f'{key}={value}\n' for key, value in values.items())


def native(options: Options) -> None:
    source = Path.cwd()
    if options.release == 'true':
        with phase('Validate release identity'):
            release_guard(ReleaseGuard(source=source))
    image = f'.#packages.{options.system}.garmin-hass-app-image'
    with phase('Build native OCI image'):
        archive = build(image)
    with phase('Build SPDX inventory'):
        sbom = build(f'.#packages.{options.system}.garmin-hass-app-sbom')
    with phase('Verify and record native build'):
        derivation = Path(run('nix', 'eval', '--raw', image + '.drvPath', '--no-update-lock-file', capture=True))
        record_build(
            NativeBuild(
                source=source,
                archive=archive,
                sbom=sbom,
                derivation=derivation,
                workflow_run=f'https://github.com/{os.environ["GITHUB_REPOSITORY"]}/actions/runs/{os.environ["GITHUB_RUN_ID"]}',
                output=Path('.tmp/hass-native-build'),
            )
        )
    architecture = 'amd64' if options.system == 'x86_64-linux' else 'aarch64'
    sources = options.release == 'true' or (options.sources == 'true' and architecture == 'amd64')
    outputs(architecture=architecture, sources=str(sources).lower())
    if sources:
        with phase('Export dependency sources'):
            metadata = Path('.tmp/dependency-metadata.json')
            metadata.write_text(
                run(
                    'nix',
                    'eval',
                    '--json',
                    f'.#legacyPackages.{options.system}.hass-source-metadata',
                    '--no-update-lock-file',
                    capture=True,
                )
            )
            bundle_sources(
                BundleSources(
                    archive=archive,
                    metadata=metadata,
                    output=Path('.tmp/hass-native-sources'),
                    realise=True,
                )
            )


def require_hosted_runner() -> None:
    if any(
        os.environ.get(name) != value
        for name, value in (
            ('GITHUB_ACTIONS', 'true'),
            ('RUNNER_ENVIRONMENT', 'github-hosted'),
            ('RUNNER_OS', 'Linux'),
        )
    ):
        raise ValueError('Registry credentials require a disposable GitHub-hosted Linux runner')


def login() -> None:
    require_hosted_runner()
    directory = Path(os.environ['DOCKER_CONFIG'])
    expected = Path.cwd() / '.tmp/docker-auth'
    if directory != expected or directory.is_symlink() or directory.exists():
        raise ValueError('Registry credentials require a fresh owned .tmp/docker-auth directory')
    if Path(os.environ['REGISTRY_AUTH_FILE']) != directory / 'config.json':
        raise ValueError('Skopeo and signing must share the owned registry credentials')
    directory.mkdir(mode=0o700, parents=True)
    previous = os.umask(0o077)
    try:
        run(
            'skopeo',
            'login',
            '--authfile',
            str(directory / 'config.json'),
            '--username',
            os.environ['GITHUB_ACTOR'],
            '--password-stdin',
            'ghcr.io',
            secret=os.environ['GH_TOKEN'],
        )
    finally:
        os.umask(previous)
    # The pinned actions/attest reader ignores DOCKER_CONFIG and uses this path.
    default = Path.home() / '.docker/config.json'
    backup = directory / 'original-config'
    default.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if default.exists() or default.is_symlink():
        shutil.move(default, backup)
    try:
        default.symlink_to(directory / 'config.json')
    except BaseException:
        if backup.exists() or backup.is_symlink():
            shutil.move(backup, default)
        raise


def cleanup() -> None:
    require_hosted_runner()
    directory = Path.cwd() / '.tmp/docker-auth'
    credentials = directory / 'config.json'
    try:
        credentials.unlink(missing_ok=True)
        default, backup = Path.home() / '.docker/config.json', directory / 'original-config'
        if default.is_symlink() and default.readlink() == credentials:
            default.unlink()
        if (backup.exists() or backup.is_symlink()) and not (default.exists() or default.is_symlink()):
            shutil.move(backup, default)
    finally:
        cleanup_staging(CleanupStaging(directory=PUBLICATION))


def acceptance(build_directory: Path, evidence: Path, *arguments: str) -> None:
    archives = list(build_directory.glob('*.oci.tar'))
    if len(archives) != 1:
        raise ValueError('Acceptance requires exactly one native OCI archive')
    run_owned(
        sys.executable,
        str(Path.cwd() / 'infra/python/haos_app_session.py'),
        'check',
        '--app-archive',
        str(archives[0]),
        '--build-receipt',
        str(build_directory / 'native-build.json'),
        '--evidence-directory',
        str(evidence),
        *arguments,
    )


def draft(*, preflight: bool = False) -> None:
    source = Path.cwd()
    with phase('Validate candidate identity'):
        if preflight:
            hass_preflight.guard(source)
        else:
            release_guard(ReleaseGuard(source=source))
    with phase('Prepare release candidate'):
        prepare = hass_preflight.prepare if preflight else prepare_publication
        prepare(
            PreparePublication(
                source=source,
                build=NATIVE,
                sources=SOURCES,
                repository=build('.#garmin-hass-app-repository'),
                output=PUBLICATION,
            )
        )
    with phase('Stage candidate image and repository'):
        login()
        stage = StagePublication(source=source, directory=PUBLICATION, build=NATIVE)
        if preflight:
            document, inputs = hass_preflight.check_candidate(source, PUBLICATION / 'candidate')
            stage_image(stage, inputs['version'], inputs['image']['index_digest'], '')
        else:
            stage_publication(stage)
    local, staged = Path('.tmp/lifecycle-acceptance'), Path('.tmp/staged-acceptance')
    with phase('Test HAOS installation and lifecycle'):
        acceptance(NATIVE, local, '--repository', str(PUBLICATION / 'repository'), '--lifecycle')
    with phase('Test staged repository installation in HAOS'):
        staging = object_value(read_json(PUBLICATION / 'staging.json'))
        acceptance(
            NATIVE,
            staged,
            '--repository-url',
            text(staging['repository_url']),
            '--registry-digest',
            text(staging['index_digest']),
            '--registry-tag',
            text(staging['tag']),
        )
    with phase('Package HAOS acceptance evidence'):
        for name in ('workflow.json', 'installation.json'):
            shutil.copyfile(local / name, staged / ('lifecycle-' + name))
        package = hass_preflight.package if preflight else package_evidence
        package(ReleaseEvidence(source=source, directory=PUBLICATION, build=NATIVE, evidence=staged))
    document = object_value(read_json(PUBLICATION / 'candidate' / ('registry.json' if preflight else 'release.json')))
    platforms = object_rows(document['platforms'])
    if len(platforms) != 1:
        raise ValueError('Experimental publication requires one native platform')
    index = text(object_value(document['image'])['index_digest'])
    manifest = text(platforms[0]['manifest_digest'])
    if any(not re.fullmatch(r'sha256:[0-9a-f]{64}', value) for value in (index, manifest)):
        raise ValueError('Invalid OCI attestation subject')
    BUNDLES.mkdir()
    outputs(
        image=IMAGE,
        index=index,
        manifest=manifest,
        index_bundle=str(BUNDLES / (index.replace(':', '-') + '.sigstore.jsonl')),
        manifest_bundle=str(BUNDLES / (manifest.replace(':', '-') + '.sigstore.jsonl')),
    )


def complete() -> None:
    if os.environ.get('GITHUB_REF', '').startswith('refs/heads/'):
        with phase('Verify signed branch candidate'):
            hass_preflight.verify(Path.cwd(), PUBLICATION, BUNDLES)
        with phase('Reject invalid branch provenance'):
            hass_preflight.qualify_rejections(Path.cwd(), PUBLICATION, BUNDLES)
    else:
        with phase('Verify independent draft downloads'):
            complete_publication(
                CompletePublication(
                    source=Path.cwd(),
                    directory=PUBLICATION,
                    build=NATIVE,
                    bundles=BUNDLES,
                    evidence=Path('.tmp/staged-acceptance'),
                )
            )


def promote() -> None:
    with phase('Validate release identity'):
        release_guard(ReleaseGuard(source=Path.cwd()))
    if PUBLICATION.exists() or PUBLICATION.is_symlink():
        raise ValueError('Promotion scratch already exists')
    with phase('Authenticate to registry'):
        login()
    previous_handler = signal.signal(signal.SIGTERM, terminated)
    try:
        with phase('Verify and promote signed draft'):
            promote_draft(PromoteDraft(source=Path.cwd(), output=PUBLICATION))
        with phase('Test published repository installation in HAOS'):
            staging = object_value(read_json(PUBLICATION / 'staging.json'))
            acceptance(
                PUBLICATION / 'native',
                Path('.tmp/final-acceptance'),
                '--repository-url',
                REMOTE + '#hass-apps',
                '--registry-digest',
                text(staging['index_digest']),
            )
        with phase('Record successful publication'):
            qualify_promotion(PUBLICATION)
    except BaseException:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        with phase('Recover unfinished promotion'):
            rollback_store(RollbackStore(directory=PUBLICATION))
        raise
    finally:
        signal.signal(signal.SIGTERM, previous_handler)


def main(options: Options) -> None:
    if options.command == 'native':
        native(options)
    elif options.command == 'draft':
        draft()
    elif options.command == 'preflight':
        draft(preflight=True)
    elif options.command == 'complete':
        complete()
    elif options.command == 'verify-preflight':
        with phase('Verify independently downloaded branch candidate'):
            hass_preflight.verify(Path.cwd(), PUBLICATION, BUNDLES)
    elif options.command == 'promote':
        promote()
    else:
        cleanup()


if __name__ == '__main__':
    main(tyro.cli(Options))
