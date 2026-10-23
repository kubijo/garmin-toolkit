"""Executable CI phases; Actions owns permissions, dependencies, signing, and artifact transport."""

import json
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
    build_directories,
    cleanup_staging,
    complete_publication,
    package_evidence,
    platform_directory,
    prepare_publication,
    promote_draft,
    qualify_promotion,
    release_guard,
    rollback_store,
    stage_image,
    stage_publication,
)
from hass_registry import PrepareRegistry, prepare_registry
from hass_release_build import NativeBuild, build_source, record_build, verify_build_inputs
from hass_repository import copy_repository
from hass_sources import BundleSources, bundle_sources
from json_data import Object, file_sha256, json_bytes, object_rows, object_value, read_json, text, unique_object
from processes import run_owned, terminated

NIX_LIMITS = ('--max-jobs', '1', '--cores', '2', '--no-update-lock-file')
PUBLICATION = Path('.tmp/publication')
NATIVE = Path('.tmp/native')
SOURCES = Path('.tmp/sources')
BUNDLES = Path('.tmp/bundles')
# Review this published baseline when preparing the next release; never resolve a moving "latest" release.
UPGRADE_BASELINE = '0.1.1'


@dataclass(frozen=True, kw_only=True)
class Options:
    command: Annotated[
        Literal[
            'native',
            'acceptance',
            'delivery',
            'draft',
            'preflight',
            'complete',
            'verify-preflight',
            'promote',
            'cleanup',
        ],
        tyro.conf.Positional,
    ]
    system: Literal['x86_64-linux', 'aarch64-linux'] = 'x86_64-linux'
    release: Literal['true', 'false'] = 'false'
    sources: Literal['true', 'false'] = 'false'
    build_directory: Path = NATIVE
    """Native acceptance inputs exported by the build job."""
    evidence_directory: Path = Path('.tmp/native-acceptance')
    """Fresh native acceptance evidence under repository .tmp."""
    image: Path | None = None
    """Cached pinned HAOS image for native acceptance; download when omitted."""
    source_run: int | None = None
    """Successful canonical CI run supplying the retained ARM64 delivery artifact."""


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
    if any(
        not re.fullmatch(r'[a-z_][a-z0-9_]*', key) or '\n' in value or '\r' in value for key, value in values.items()
    ):
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
    sources = options.release == 'true' or options.sources == 'true'
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


def acceptance(build_directory: Path, evidence: Path, *arguments: str, timeout: float = 7200) -> None:
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
        timeout=timeout,
    )


def native_acceptance(options: Options) -> None:
    architecture = 'aarch64' if options.system == 'aarch64-linux' else 'amd64'
    archives = list(options.build_directory.glob('*.oci.tar'))
    if len(archives) != 1:
        raise ValueError('Native acceptance requires exactly one exported OCI archive')
    with phase('Verify native acceptance inputs'):
        receipt = verify_build_inputs(archives[0], options.build_directory / 'native-build.json', Path.cwd())
        if object_value(receipt['platform']).get('architecture') != architecture:
            raise ValueError('Native acceptance platform differs from the selected guest architecture')
    with phase('Prepare private acceptance metadata'):
        repository = build(
            '.#garmin-hass-arm64-test-repository' if architecture == 'aarch64' else '.#garmin-hass-app-repository'
        )
    arguments = ['--arch', architecture, '--repository', str(repository), '--lifecycle', '--timeout', '1800']
    if options.image is not None:
        arguments.extend(('--image', str(options.image)))
    with phase('Test native image installation and lifecycle in HAOS'):
        acceptance(options.build_directory, options.evidence_directory, *arguments, timeout=1900)


def download_delivery_build(options: Options) -> Object:
    if options.source_run is None or options.source_run <= 0:
        raise ValueError('Delivery requires a positive source CI run ID')
    if options.build_directory.exists() or options.build_directory.is_symlink():
        raise ValueError('Delivery requires a fresh native artifact directory')
    run_id = str(options.source_run)
    document = object_value(
        json.loads(
            run('gh', 'api', 'repos/kubijo/garmin-toolkit/actions/runs/' + run_id, capture=True),
            object_pairs_hook=unique_object,
        )
    )
    head = text(document.get('head_sha'))
    if (
        document.get('id') != options.source_run
        or document.get('path') != '.github/workflows/ci.yml'
        or document.get('event') not in ('push', 'pull_request', 'workflow_dispatch')
        or document.get('status') != 'completed'
        or document.get('conclusion') != 'success'
        or object_value(document.get('repository')).get('full_name') != 'kubijo/garmin-toolkit'
        or not re.fullmatch(r'[0-9a-f]{40}', head)
    ):
        raise ValueError('Delivery requires a successful canonical CI run from this repository')
    run('git', 'merge-base', '--is-ancestor', head, 'HEAD')
    run(
        'gh',
        'run',
        'download',
        run_id,
        '--repo',
        'kubijo/garmin-toolkit',
        '--name',
        'garmin-hass-native-aarch64',
        '--dir',
        str(options.build_directory),
    )
    receipt = object_value(read_json(options.build_directory / 'native-build.json'))
    platform = object_value(receipt.get('platform'))
    commit = text(object_value(receipt.get('source')).get('commit'))
    if (
        platform.get('workflow_run') != REMOTE + '/actions/runs/' + run_id
        or platform.get('architecture') != 'aarch64'
        or not re.fullmatch(r'[0-9a-f]{40}', commit)
    ):
        raise ValueError('Native receipt differs from the selected ARM64 CI run')
    run('git', 'fetch', '--no-tags', '--no-write-fetch-head', REMOTE, commit)
    if commit != head:
        parents = run('git', 'show', '-s', '--format=%P', commit, capture=True).split()
        if document['event'] != 'pull_request' or len(parents) != 2 or parents[1] != head:
            raise ValueError('Native receipt source is not the selected CI head or its tested PR merge')
    archives = list(options.build_directory.glob('*.oci.tar'))
    if len(archives) != 1:
        raise ValueError('Delivery requires exactly one exported native OCI archive')
    return verify_build_inputs(archives[0], options.build_directory / 'native-build.json', Path.cwd())


def verify_delivery(evidence: Path, receipt: Object, staging: Object) -> None:
    workflow = object_value(read_json(evidence / 'workflow.json'))
    installed = object_value(read_json(evidence / 'installation.json'))
    platform = object_value(receipt['platform'])
    reboot = object_value(workflow.get('haos_reboot'))
    if (
        any(
            workflow.get(key) is not True
            for key in ('passed', 'restart', 'cold_restore', 'post_backup_mutation_reverted')
        )
        or not reboot.get('before')
        or not reboot.get('after')
        or reboot['before'] == reboot['after']
        or read_json(evidence / 'native-build.json') != receipt
        or any(installed.get(key) != platform[key] for key in ('manifest_digest', 'config_digest', 'archive_sha256'))
        or installed.get('architecture') != 'arm64'
        or installed.get('archive_version') != receipt['version']
        or installed.get('version') != staging['tag']
        or installed.get('repository_url') != staging['repository_url']
        or installed.get('registry_digest') != staging['index_digest']
    ):
        raise ValueError('HAOS delivery evidence differs from the staged ARM64 image or retained native build')


def delivery(options: Options) -> None:
    require_hosted_runner()
    version, identity = build_source(Path.cwd())
    if os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch' or os.environ.get('GITHUB_SHA') != identity['commit']:
        raise ValueError('Delivery requires a manual workflow run at its exact clean checkout commit')
    if options.system != 'aarch64-linux' or any(os.environ.get(name) != 'false' for name in ('PUBLISH', 'RECOVERY')):
        raise ValueError('ARM64 delivery is separate from publication and recovery modes')
    with phase('Admit retained ARM64 CI artifact'):
        receipt = download_delivery_build(options)
        if receipt['version'] != version:
            raise ValueError('Retained native image version differs from the dispatched source')
    protected = ('refs/heads/hass-apps', 'refs/heads/hass-publication', 'refs/tags/v*')
    before = run('git', 'ls-remote', '--refs', REMOTE, *protected, capture=True)
    with phase('Prepare and stage ARM64 delivery candidate'):
        if PUBLICATION.exists() or PUBLICATION.is_symlink():
            raise ValueError('Delivery requires a fresh publication directory')
        document = prepare_registry(
            PrepareRegistry(source=Path.cwd(), builds=[options.build_directory], output=PUBLICATION / 'registry')
        )
        copy_repository(build('.#garmin-hass-arm64-test-repository'), PUBLICATION / 'repository')
        login()
        stage_image(
            StagePublication(source=Path.cwd(), directory=PUBLICATION, build=options.build_directory),
            version,
            text(object_value(document['image'])['index_digest']),
            '',
        )
    staging = object_value(read_json(PUBLICATION / 'staging.json'))
    arguments = [
        '--arch',
        'aarch64',
        '--repository-url',
        text(staging['repository_url']),
        '--registry-digest',
        text(staging['index_digest']),
        '--registry-tag',
        text(staging['tag']),
        '--timeout',
        '1800',
    ]
    if options.image is not None:
        arguments.extend(('--image', str(options.image)))
    with phase('Verify staged ARM64 Git and GHCR installation in HAOS'):
        acceptance(options.build_directory, options.evidence_directory, *arguments, timeout=1900)
        verify_delivery(options.evidence_directory, receipt, staging)
    if run('git', 'ls-remote', '--refs', REMOTE, *protected, capture=True) != before:
        raise RuntimeError('Published HASS refs changed during ARM64 delivery qualification')
    (PUBLICATION / 'delivery.json').write_bytes(
        json_bytes(
            {
                'passed': True,
                'source_run': options.source_run,
                'source': receipt['source'],
                'platform': receipt['platform'],
                'staging': staging,
                'published_refs_unchanged': True,
                'workflow_sha256': file_sha256(options.evidence_directory / 'workflow.json'),
                'installation_sha256': file_sha256(options.evidence_directory / 'installation.json'),
            }
        )
    )


def download_upgrade_baseline() -> Path:
    directory = Path('.tmp/published-baseline')
    directory.mkdir()
    arguments = [
        'gh',
        'release',
        'download',
        'v' + UPGRADE_BASELINE,
        '--repo',
        'kubijo/garmin-toolkit',
        '--dir',
        str(directory),
    ]
    for name in (
        'release.json',
        'SHA256SUMS',
        'checksums.sigstore.jsonl',
        'artifacts.sigstore.jsonl',
        f'garmin-toolkit-hass-{UPGRADE_BASELINE}-amd64.oci.tar',
    ):
        arguments.extend(('--pattern', name))
    run(*arguments)
    return directory


def publication_acceptance(directory: Path, evidence: Path, architecture: str, *arguments: str) -> None:
    budget = ('--timeout', '1800') if architecture == 'aarch64' else ()
    acceptance(
        directory,
        evidence,
        '--arch',
        architecture,
        *arguments,
        *budget,
        timeout=1900 if architecture == 'aarch64' else 7200,
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
    with phase('Download published upgrade baseline'):
        baseline = download_upgrade_baseline()
    document = object_value(read_json(PUBLICATION / 'candidate' / ('registry.json' if preflight else 'release.json')))
    platforms = object_rows(document['platforms'])
    if sorted(text(platform['architecture']) for platform in platforms) != ['aarch64', 'amd64']:
        raise ValueError('New publication requires both qualified native platforms')
    staging = object_value(read_json(PUBLICATION / 'staging.json'))
    for platform in platforms:
        architecture = text(platform['architecture'])
        native = platform_directory(NATIVE, architecture)
        local_evidence, staged_evidence = local / architecture, staged / architecture
        # No published ARM64 baseline exists yet. Its fixture reports migration_compatibility=false.
        upgrade = ('--baseline-release', str(baseline)) if architecture == 'amd64' else ()
        with phase('Test ' + architecture + ' HAOS installation and lifecycle'):
            publication_acceptance(
                native,
                local_evidence,
                architecture,
                '--repository',
                str(PUBLICATION / 'repository'),
                '--lifecycle',
                *upgrade,
            )
        with phase('Test ' + architecture + ' staged repository installation in HAOS'):
            publication_acceptance(
                native,
                staged_evidence,
                architecture,
                '--repository-url',
                text(staging['repository_url']),
                '--registry-digest',
                text(staging['index_digest']),
                '--registry-tag',
                text(staging['tag']),
            )
        for name in ('workflow.json', 'installation.json'):
            shutil.copyfile(local_evidence / name, staged_evidence / ('lifecycle-' + name))
    with phase('Package HAOS acceptance evidence'):
        package = hass_preflight.package if preflight else package_evidence
        package(ReleaseEvidence(source=source, directory=PUBLICATION, build=NATIVE, evidence=staged))
    index = text(object_value(document['image'])['index_digest'])
    manifests = {text(platform['architecture']): text(platform['manifest_digest']) for platform in platforms}
    if any(not re.fullmatch(r'sha256:[0-9a-f]{64}', value) for value in (index, *manifests.values())):
        raise ValueError('Invalid OCI attestation subject')
    BUNDLES.mkdir()
    outputs(
        image=IMAGE,
        index=index,
        manifest=manifests['amd64'],
        index_bundle=str(BUNDLES / (index.replace(':', '-') + '.sigstore.jsonl')),
        manifest_bundle=str(BUNDLES / (manifests['amd64'].replace(':', '-') + '.sigstore.jsonl')),
        arm64_manifest=manifests['aarch64'],
        arm64_manifest_bundle=str(BUNDLES / (manifests['aarch64'].replace(':', '-') + '.sigstore.jsonl')),
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
            directories = build_directories(PUBLICATION / 'native')
            for native in directories:
                architecture = 'aarch64' if native.name == 'aarch64' else 'amd64'
                evidence = Path('.tmp/final-acceptance')
                publication_acceptance(
                    native,
                    evidence / architecture if len(directories) > 1 else evidence,
                    architecture,
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
    elif options.command == 'acceptance':
        native_acceptance(options)
    elif options.command == 'delivery':
        delivery(options)
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
