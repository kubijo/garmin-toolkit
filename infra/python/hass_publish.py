"""Prepare and promote a GitHub-only HASS release without replacing published version bytes."""

import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import yaml

from archive_io import chunks, open_tar
from hass_attestations import VerifyProvenance, attest
from hass_registry import PrepareRegistry, VerifyRegistry, prepare_registry, verify_registry
from hass_release_build import build_source
from hass_release_contract import (
    Assembly,
    Payload,
    Platform,
    Release,
    check_directory,
    check_shape,
    file_sha256,
    inventory,
    validate_document,
)
from hass_release_source import admit_source
from hass_repository import copy_repository
from hass_source_parts import part_path
from hass_sources import source_part_count, verify_sources
from hass_store import FENCE, Store, oid, write_journal
from json_data import Object, json_bytes, object_rows, object_value, read_json, text

REPOSITORY = 'kubijo/garmin-toolkit'
REMOTE = 'https://github.com/' + REPOSITORY
IMAGE = 'ghcr.io/kubijo/garmin-toolkit-hass'


@dataclass(frozen=True, kw_only=True)
class ReleaseGuard:
    """Require a tag workflow running the same clean commit reachable from main."""

    source: Path


@dataclass(frozen=True, kw_only=True)
class PreparePublication:
    """Assemble an AMD64 candidate and its unchanged OCI index; publish nothing."""

    source: Path
    build: Path
    sources: Path
    repository: Path
    output: Path


@dataclass(frozen=True, kw_only=True)
class StagePublication:
    """Push a uniquely named image tag and repository branch for anonymous HAOS acceptance."""

    source: Path
    directory: Path
    build: Path


@dataclass(frozen=True, kw_only=True)
class CompletePublication(StagePublication):
    """Verify signed, remotely tested artifacts and independent draft release downloads."""

    bundles: Path
    evidence: Path


@dataclass(frozen=True, kw_only=True)
class ReleaseEvidence(StagePublication):
    """Package only the synthetic acceptance reports and app-owned dark screenshots."""

    evidence: Path


@dataclass(frozen=True, kw_only=True)
class RollbackStore:
    """Recover this attempt's unfinished store changes using an exact branch lease."""

    directory: Path


@dataclass(frozen=True, kw_only=True)
class CleanupStaging(RollbackStore):
    """Recover unfinished store changes and remove this attempt's unchanged candidate branch."""


@dataclass(frozen=True, kw_only=True)
class PromoteDraft:
    """Authenticate existing release assets, then promote their original tested bytes."""

    source: Path
    output: Path


@dataclass(frozen=True, kw_only=True)
class WithdrawPublication:
    """Remove a qualified release from the store using exact store/state leases; retain all release artifacts."""

    version: str
    store_commit: str
    state_commit: str
    reason: str
    output: Path


EVIDENCE_FILES = (
    'workflow.json',
    'installation.json',
    'native-build.json',
    'versions.json',
    'populated-course.png',
    'restarted-course.png',
    'rebooted-course.png',
    'restored-course.png',
    'lifecycle-workflow.json',
    'lifecycle-installation.json',
)


def command(*arguments: str, cwd: Path | None = None) -> str:
    return subprocess.run(
        arguments,
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
        timeout=630,
        env={**os.environ, 'GH_HOST': 'github.com'} if arguments[0] == 'gh' else None,
    ).stdout.strip()


def image_manifest(reference: str) -> bytes:
    return subprocess.run(
        ['skopeo', 'inspect', '--raw', '--no-creds', '--tls-verify=true', 'docker://' + reference],
        check=True,
        capture_output=True,
        timeout=60,
    ).stdout


def image_digest(reference: str) -> str:
    return 'sha256:' + hashlib.sha256(image_manifest(reference)).hexdigest()


def release_guard(options: ReleaseGuard) -> tuple[str, Object]:
    version, identity = build_source(options.source)
    reference, commit = os.environ.get('GITHUB_REF'), os.environ.get('GITHUB_SHA')
    if (
        os.environ.get('GITHUB_REPOSITORY') != REPOSITORY
        or reference != 'refs/tags/v' + version
        or commit != identity['commit']
        or identity['tag'] != 'v' + version
    ):
        raise ValueError('Run the release workflow from the exact vVERSION tag in ' + REPOSITORY)
    document = cast(
        Assembly,
        {
            'version': version,
            'source': identity,
            'image': {'repository': IMAGE},
            'platforms': [],
        },
    )
    admit_source(options.source, document)
    return version, identity


def prepare_publication(options: PreparePublication) -> None:
    from hass_release import Assemble, assemble

    release_guard(ReleaseGuard(source=options.source))
    check_shape(options.repository, 'hass-app-repository.dirspec.toml')
    if options.repository.is_symlink() or any(path.is_symlink() for path in options.repository.rglob('*')):
        raise ValueError('Release repository metadata must contain regular files and directories')
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Publication output already exists')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-publication.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'publication'
        staged.mkdir()
        registry = prepare_registry(
            PrepareRegistry(source=options.source, builds=[options.build], output=staged / 'registry')
        )
        if [row.get('architecture') for row in object_rows(registry['platforms'])] != ['amd64']:
            raise ValueError('Only AMD64 has experimental release runtime qualification')
        document = cast(
            Assembly, {key: registry[key] for key in ('schema_version', 'version', 'source', 'image', 'platforms')}
        )
        document['source_parts'] = {
            'amd64': source_part_count(object_value(read_json(options.sources / 'dependency-sources.json')))
        }
        (staged / 'assembly.json').write_bytes(json_bytes(document))
        artifacts = staged / 'artifacts'
        copy_platform_files(options.build, options.sources, document, artifacts)
        assemble(
            Assemble(
                source=options.source,
                receipt=staged / 'assembly.json',
                artifacts=artifacts,
                output=staged / 'candidate',
            )
        )
        shutil.rmtree(artifacts)
        copy_repository(options.repository, staged / 'repository')
        staged.rename(options.output)


def copy_platform_files(build: Path, sources: Path, document: Payload, output: Path) -> None:
    count = document.get('source_parts', {}).get('amd64', 1)
    check_directory(
        sources,
        {'dependency-sources.json'}
        | {part_path(Path('dependency-sources.tar.gz'), number).name for number in range(1, count + 1)},
    )
    output.mkdir()
    for spec in inventory(document):
        if 'architecture' not in spec:
            continue
        role = spec['role']
        origin = build / spec['name']
        if role in ('dependency_sources', 'source_inventory'):
            origin = sources / (
                'dependency-sources.tar.gz' if role == 'dependency_sources' else 'dependency-sources.json'
            )
        elif role == 'dependency_sources_part':
            origin = sources / ('dependency-sources.tar.gz' + Path(spec['name']).suffix)
        shutil.copyfile(origin, output / spec['name'], follow_symlinks=False)


def checked_candidate(source: Path, directory: Path) -> Release:
    from hass_release import Verify, verify

    version, identity = release_guard(ReleaseGuard(source=source))
    candidate = directory / 'candidate'
    document = verify(
        Verify(
            directory=candidate,
            manifest_sha256=file_sha256(candidate / 'release.json'),
            version=version,
            commit=text(identity['commit']),
        )
    )
    if [row['architecture'] for row in document['platforms']] != ['amd64']:
        raise ValueError('Only qualified AMD64 releases may be published')
    roles = {entry['role']: candidate / entry['name'] for entry in document['files']}
    verify_sources(
        roles['dependency_sources'],
        roles['source_inventory'],
        document['platforms'][0]['archive_sha256'],
        require_resolved=True,
        part_count=document.get('source_parts', {}).get('amd64', 1),
    )
    return document


def git(*arguments: str, cwd: Path | None = None) -> str:
    return command(
        'git',
        '-c',
        'credential.helper=',
        '-c',
        'credential.helper=!gh auth git-credential',
        '-c',
        'core.hooksPath=/dev/null',
        '-c',
        'commit.gpgsign=false',
        *arguments,
        cwd=cwd,
    )


def branch_head(branch: str) -> str:
    result = git('ls-remote', '--refs', REMOTE, 'refs/heads/' + branch)
    if not result:
        return ''
    digest, _, reference = result.partition('\t')
    if not re.fullmatch(r'[0-9a-f]{40}', digest) or reference != 'refs/heads/' + branch:
        raise ValueError('Unexpected remote branch response')
    return digest


def assert_remote_tag(version: str, commit: str) -> None:
    reference = 'refs/tags/v' + version
    rows = git('ls-remote', '--tags', REMOTE, reference, reference + '^{}').splitlines()
    targets: dict[str, str] = {}
    for row in rows:
        digest, _, name = row.partition('\t')
        if name not in (reference, reference + '^{}') or not re.fullmatch(r'[0-9a-f]{40}', digest):
            raise ValueError('Unexpected remote release tag response')
        targets[name] = digest
    if targets.get(reference + '^{}', targets.get(reference)) != commit:
        raise ValueError('Remote release tag moved or disappeared; refusing publication')


def push_metadata(
    directory: Path,
    branch: str,
    previous: str,
    *,
    version: str | None = None,
    resume: bool = False,
    before_push: Callable[[str, str, Path], tuple[str, str] | None] | None = None,
) -> str:
    with tempfile.TemporaryDirectory(prefix='.hass-metadata.', dir=directory.parent) as temporary:
        checkout = Path(temporary) / 'repository'
        copy_repository(directory, checkout)
        if version is not None:
            path = checkout / 'garmin_toolkit/config.yaml'
            config = object_value(yaml.safe_load(path.read_text()))
            config['version'] = version
            path.write_text(yaml.safe_dump(config, sort_keys=False))
        git('init', '-q', '--initial-branch=' + branch, str(checkout))
        if previous:
            git('fetch', '--no-tags', '--depth=1', REMOTE, previous, cwd=checkout)
            git('update-ref', 'HEAD', previous, cwd=checkout)
        git('add', '.', cwd=checkout)
        git(
            '-c',
            'user.name=Garmin release',
            '-c',
            'user.email=release@users.noreply.github.com',
            'commit',
            '-qm',
            'Publish verified HASS metadata',
            '--allow-empty',
            cwd=checkout,
        )
        commit = git('rev-parse', 'HEAD', cwd=checkout)
        current = branch_head(branch) if resume else previous
        if current != previous:
            git('fetch', '--no-tags', '--depth=2', REMOTE, current, cwd=checkout)
            if git('show', '-s', '--format=%P', current, cwd=checkout) != previous or git(
                'rev-parse', current + '^{tree}', cwd=checkout
            ) != git('rev-parse', 'HEAD^{tree}', cwd=checkout):
                raise ValueError('Store advertisement changed; refusing to replace unrelated metadata')
            commit = current
        fence = before_push(commit, current, checkout) if before_push is not None else None
        git(
            'push',
            *(['--atomic', '--force-with-lease=refs/heads/' + FENCE + ':' + fence[0]] if fence else []),
            '--force-with-lease=refs/heads/' + branch + ':' + current,
            REMOTE,
            commit + ':refs/heads/' + branch,
            *([fence[1] + ':refs/heads/' + FENCE] if fence else []),
            cwd=checkout,
        )
        return commit


def staging_target(version: str) -> tuple[str, str]:
    run, attempt = os.environ.get('GITHUB_RUN_ID', ''), os.environ.get('GITHUB_RUN_ATTEMPT', '')
    if not re.fullmatch(r'[1-9][0-9]*', run) or not re.fullmatch(r'[1-9][0-9]*', attempt):
        raise ValueError('Staging requires the actual GitHub workflow run and attempt')
    tag = f'{version}-candidate.{run}.{attempt}'
    branch = f'hass-candidate-{run}-{attempt}'
    if branch_head(branch):
        raise ValueError('Candidate branch already exists; retain its original bytes')
    return tag, branch


def stage_publication(options: StagePublication) -> None:
    document = checked_candidate(options.source, options.directory)
    assert_remote_tag(document['version'], document['source']['commit'])
    tag, branch = staging_target(document['version'])
    store().reconcile(options.directory)
    _stage_image(options, tag, branch, document['image']['index_digest'], branch_head('hass-apps'))


def stage_image(options: StagePublication, version: str, digest: str, previous: str) -> None:
    tag, branch = staging_target(version)
    _stage_image(options, tag, branch, digest, previous)


def _stage_image(options: StagePublication, tag: str, branch: str, digest: str, previous: str) -> None:
    command(
        'skopeo',
        '--command-timeout',
        '10m',
        'copy',
        '--all',
        '--preserve-digests',
        '--image-parallel-copies',
        '1',
        'oci:' + str(options.directory / 'registry/oci'),
        'docker://' + IMAGE + ':' + tag,
    )
    try:
        verify_registry(
            VerifyRegistry(
                source=options.source,
                builds=[options.build],
                reference=IMAGE + '@' + digest,
                output=options.directory / 'staged-registry',
            )
        )
    except subprocess.CalledProcessError as error:
        raise ValueError('Anonymous staged verification failed. The GHCR package must allow public pulls.') from error
    if image_digest(IMAGE + ':' + tag) != digest:
        raise ValueError('Staged tag differs from the verified index')
    staged_commit = push_metadata(options.directory / 'repository', branch, '', version=tag)
    (options.directory / 'staging.json').write_bytes(
        json_bytes(
            {
                'tag': tag,
                'branch': branch,
                'previous': previous,
                'index_digest': digest,
                'repository_url': REMOTE + '#' + branch,
                'commit': staged_commit,
            }
        )
    )


def verify_acceptance(evidence: Path, document: Release, staging: Object) -> None:
    check_acceptance(evidence, document['platforms'][0], document['source'], document['image']['index_digest'], staging)


def check_acceptance(evidence: Path, platform: Platform, source: object, digest: str, staging: Object) -> None:
    workflow = object_value(read_json(evidence / 'workflow.json'))
    installed = object_value(read_json(evidence / 'installation.json'))
    native = object_value(read_json(evidence / 'native-build.json'))
    local = object_value(read_json(evidence / 'lifecycle-workflow.json'))
    local_image = object_value(read_json(evidence / 'lifecycle-installation.json'))
    lifecycle = object_value(local.get('lifecycle'))
    reboot = object_value(workflow.get('haos_reboot'))
    if (
        workflow.get('passed') is not True
        or workflow.get('restart') is not True
        or workflow.get('cold_restore') is not True
        or workflow.get('post_backup_mutation_reverted') is not True
        or not reboot.get('before')
        or not reboot.get('after')
        or reboot.get('before') == reboot.get('after')
        or installed.get('registry_digest') != digest
        or installed.get('manifest_digest') != platform['manifest_digest']
        or installed.get('config_digest') != platform['config_digest']
        or installed.get('archive_sha256') != platform['archive_sha256']
        or installed.get('repository_url') != staging['repository_url']
        or native.get('source') != source
        or native.get('platform') != platform
        or local.get('passed') is not True
        or any(
            lifecycle.get(key) is not True
            for key in ('upgrade', 'recovery', 'app_data_removed', 'reinstall_empty', 'backup_retained')
        )
        or any(local_image.get(key) != platform[key] for key in ('manifest_digest', 'config_digest', 'archive_sha256'))
    ):
        raise ValueError('HAOS acceptance does not qualify this staged release index')


def require_rejection(action: Callable[[], object]) -> None:
    try:
        action()
    except ValueError, subprocess.CalledProcessError, FileNotFoundError:
        return
    raise ValueError('Provenance verifier accepted a deliberately invalid release')


def qualify_rejections(options: VerifyProvenance, document: Release) -> None:
    from hass_release import verify_provenance

    wrong_commit = ('0' if options.commit[0] != '0' else '1') + options.commit[1:]
    wrong_identity = REMOTE + '/.github/workflows/untrusted.yml@refs/tags/v' + options.version
    require_rejection(
        lambda: verify_provenance(
            VerifyProvenance(
                directory=options.directory,
                bundles=options.bundles,
                version=options.version,
                commit=wrong_commit,
            )
        )
    )
    require_rejection(
        lambda: command(
            'gh',
            'attestation',
            'verify',
            str(options.directory / 'SHA256SUMS'),
            '--bundle',
            str(options.bundles / 'checksums.sigstore.jsonl'),
            '--repo',
            REPOSITORY,
            '--cert-identity',
            wrong_identity,
        )
    )
    platform = document['platforms'][0]['manifest_digest']
    index = document['image']['index_digest']
    raw = image_manifest(IMAGE + '@' + index)
    if 'sha256:' + hashlib.sha256(raw).hexdigest() != index or index == platform:
        raise ValueError('Wrong-subject qualification requires the existing, distinct release index')
    with tempfile.TemporaryDirectory(prefix='.hass-subject.', dir=options.directory.parent) as temporary:
        subject = Path(temporary) / 'index.json'
        subject.write_bytes(raw)
        attest(options, str(subject), index.replace(':', '-') + '.sigstore.jsonl')
        require_rejection(lambda: attest(options, str(subject), platform.replace(':', '-') + '.sigstore.jsonl'))
    for mode in ('altered-payload', 'missing-checksums', 'missing-manifest', 'altered-platform'):
        with tempfile.TemporaryDirectory(prefix='.hass-rejection.', dir=options.directory.parent) as temporary:
            candidate = Path(temporary)
            for path in options.directory.iterdir():
                os.link(path, candidate / path.name)
            name = 'COPYRIGHT.md'
            if mode in ('missing-manifest', 'altered-platform'):
                name = 'release.json'
            elif mode == 'missing-checksums':
                name = 'SHA256SUMS'
            target = candidate / name
            target.unlink()
            if mode == 'altered-platform':
                altered = json.loads(json_bytes(document))
                altered['platforms'][0]['manifest_digest'] = 'sha256:' + '0' * 64
                target.write_bytes(json_bytes(altered))
            elif mode == 'altered-payload':
                target.write_text('Deliberately altered payload\n')
            require_rejection(
                lambda candidate=candidate: verify_provenance(
                    VerifyProvenance(
                        directory=candidate,
                        bundles=options.bundles,
                        version=options.version,
                        commit=options.commit,
                    )
                )
            )


def package_evidence(options: ReleaseEvidence) -> None:
    document = checked_candidate(options.source, options.directory)
    verify_acceptance(options.evidence, document, object_value(read_json(options.directory / 'staging.json')))
    archive_evidence(options.directory, options.evidence)


def archive_evidence(directory: Path, evidence: Path) -> None:
    with tarfile.open(directory / 'haos-acceptance.tar.gz', 'x:gz') as archive:
        for name in EVIDENCE_FILES:
            path = evidence / name
            if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 16 * 1024**2:
                raise ValueError('Missing or oversized acceptance report/capture: ' + name)
            archive.add(path, arcname=name)
        archive.add(directory / 'staging.json', arcname='staging.json')


def extract_evidence(acceptance: Path, evidence: Path) -> Object:
    evidence.mkdir()
    seen: set[str] = set()
    with open_tar(acceptance, max_bytes=160 * 1024**2) as archive:
        for member in archive:
            if (
                member.name not in (*EVIDENCE_FILES, 'staging.json')
                or member.name in seen
                or not member.isfile()
                or not 0 < member.size <= 16 * 1024**2
            ):
                raise ValueError('Unexpected, duplicate, or oversized signed acceptance member')
            seen.add(member.name)
            with archive.extractfile(member) as source, (evidence / member.name).open('xb') as destination:
                for chunk in chunks(source):
                    destination.write(chunk)
    if seen != {*EVIDENCE_FILES, 'staging.json'}:
        raise ValueError('Signed acceptance archive is incomplete')
    return object_value(read_json(evidence / 'staging.json'))


def complete_publication(options: CompletePublication) -> None:
    from hass_release import verify_provenance

    document = checked_candidate(options.source, options.directory)
    assert_remote_tag(document['version'], document['source']['commit'])
    staging = object_value(read_json(options.directory / 'staging.json'))
    verify_acceptance(options.evidence, document, staging)
    provenance = VerifyProvenance(
        directory=options.directory / 'candidate',
        bundles=options.bundles,
        version=document['version'],
        commit=document['source']['commit'],
    )
    verify_provenance(provenance)
    qualify_rejections(provenance, document)
    (options.directory / 'provenance-qualification.json').write_bytes(
        json_bytes(
            {
                'passed': True,
                'source_commit': document['source']['commit'],
                'index_digest': document['image']['index_digest'],
                'rejections': [
                    'wrong-commit',
                    'wrong-signer',
                    'wrong-platform-subject',
                    'altered-payload',
                    'missing-checksums',
                    'missing-manifest',
                    'altered-platform',
                ],
            }
        )
    )
    acceptance = options.directory / 'haos-acceptance.tar.gz'
    attest(provenance, str(acceptance), 'acceptance.sigstore.jsonl')
    tags = object_value(json.loads(command('skopeo', 'list-tags', 'docker://' + IMAGE)))
    if (
        document['version'] in cast(list[str], tags.get('Tags', []))
        and image_digest(IMAGE + ':' + document['version']) != document['image']['index_digest']
    ):
        raise ValueError('Published image version already exists; refusing to replace it')
    tag = document['source']['tag']
    notes = (
        f'AMD64 experimental Home Assistant release. '
        f'[Installation and recovery]({REMOTE}/blob/{tag}/apps/garmin-hass/addon/DOCS.md).\n\n'
        'Verify the signed release inventory before installation. '
        'Runtime versions, acceptance reports, and dark app captures are in haos-acceptance.tar.gz. '
        'Dependency sources, notices, checksums, and attestation bundles are attached.'
    )
    command(
        'gh',
        'release',
        'create',
        tag,
        '--repo',
        REPOSITORY,
        '--verify-tag',
        '--draft',
        '--prerelease',
        '--title',
        'Garmin Toolkit ' + tag + ' — Home Assistant experimental',
        '--notes',
        notes,
    )
    assets = [*sorted(provenance.directory.iterdir()), *sorted(provenance.bundles.iterdir()), acceptance]
    command('gh', 'release', 'upload', tag, '--repo', REPOSITORY, *map(str, assets))
    with tempfile.TemporaryDirectory(prefix='.hass-download.', dir=options.directory.parent) as temporary:
        downloaded = Path(temporary)
        command('gh', 'release', 'download', tag, '--repo', REPOSITORY, '--dir', str(downloaded))
        payload, bundles = downloaded / 'payload', downloaded / 'bundles'
        payload.mkdir()
        bundles.mkdir()
        expected = {path.name for path in assets}
        if {path.name for path in downloaded.iterdir()} != expected | {'payload', 'bundles'}:
            raise ValueError('Downloaded draft release asset inventory differs')
        for path in assets:
            fetched = downloaded / path.name
            if file_sha256(fetched) != file_sha256(path):
                raise ValueError('Downloaded release bytes differ: ' + path.name)
            if path.parent == provenance.directory:
                fetched.rename(payload / path.name)
            elif path.parent == provenance.bundles:
                fetched.rename(bundles / path.name)
        independent = VerifyProvenance(
            directory=payload, bundles=bundles, version=provenance.version, commit=provenance.commit
        )
        verify_provenance(independent)
        attest(independent, str(downloaded / acceptance.name), 'acceptance.sigstore.jsonl')


def store() -> Store:
    return Store(REMOTE, git, branch_head)


def withdraw_publication(options: WithdrawPublication) -> None:
    previous, before = oid(options.store_commit), oid(options.state_commit)
    if not re.fullmatch(r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)', options.version):
        raise ValueError('Withdrawal requires a numeric release version')
    if not options.reason.strip() or len(options.reason) > 2000:
        raise ValueError('Withdrawal requires a nonempty reason of at most 2000 characters')
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Withdrawal output already exists; retain its original journal')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    owner = store()
    with tempfile.TemporaryDirectory(prefix='.hass-withdrawal.', dir=options.output.parent) as temporary:
        checkout = Path(temporary)
        git('init', '-q', str(checkout))
        current, fence = branch_head('hass-apps'), branch_head(FENCE)
        state = owner.state(checkout, fence) if fence else {}
        if (
            state.get('status') == 'withdrawn'
            and state.get('previous') == previous
            and state.get('published') == current
            and state.get('version') == options.version
            and state.get('reason') == options.reason
            and git('show', '-s', '--format=%P', fence, cwd=checkout) == before
        ):
            report: Object = {**state, 'fence_before': before, 'fence_withdrawn': fence, 'completed': True}
        else:
            if (current, fence) != (previous, before):
                raise ValueError('Store or publication state changed; review fresh heads before withdrawal')
            if state.get('status') != 'qualified' or state.get('published') != previous:
                raise ValueError(
                    'Withdrawal requires the current, durably qualified release; recover unfinished work first'
                )
            owner.fetch(checkout, previous)
            config = object_value(yaml.safe_load(git('show', previous + ':garmin_toolkit/config.yaml', cwd=checkout)))
            if config.get('version') != options.version or config.get('slug') != 'garmin_toolkit':
                raise ValueError('Store app identity/version differs from the requested withdrawal')
            git('read-tree', previous, cwd=checkout)
            git('rm', '-r', '--cached', '--', 'garmin_toolkit', cwd=checkout)
            tree = git('write-tree', cwd=checkout)
            published = git(
                '-c',
                'user.name=Garmin release',
                '-c',
                'user.email=release@users.noreply.github.com',
                'commit-tree',
                tree,
                '-p',
                previous,
                '-m',
                'Withdraw HASS release ' + options.version,
                cwd=checkout,
            )
            withdrawal: Object = {
                'status': 'withdrawn',
                'previous': previous,
                'published': published,
                'version': options.version,
                'reason': options.reason,
            }
            withdrawn = owner.commit(checkout, withdrawal, before)
            report = {**withdrawal, 'fence_before': before, 'fence_withdrawn': withdrawn, 'completed': False}
            options.output.mkdir()
            write_journal(options.output, report, name='withdrawal.json')
            owner.push(checkout, before, withdrawn, advertisement=(previous, published))
            write_journal(options.output, {**report, 'completed': True}, replace=True, name='withdrawal.json')
            return
        options.output.mkdir()
        write_journal(options.output, report, name='withdrawal.json')


def qualify_promotion(directory: Path) -> None:
    store().qualify(directory)


def promote_candidate(options: StagePublication, document: Release, staging: Object) -> None:
    assert_remote_tag(document['version'], document['source']['commit'])
    store().reconcile(options.directory)
    tags = object_value(json.loads(command('skopeo', 'list-tags', 'docker://' + IMAGE)))
    if (
        document['version'] in cast(list[str], tags.get('Tags', []))
        and image_digest(IMAGE + ':' + document['version']) != document['image']['index_digest']
    ):
        raise ValueError('Published image version already exists; refusing to replace it')

    def publish(commit: str, current: str, checkout: Path) -> tuple[str, str] | None:
        fence = store().prepare(checkout, options.directory, current, commit)
        command(
            'skopeo',
            '--command-timeout',
            '10m',
            'copy',
            '--all',
            '--preserve-digests',
            '--image-parallel-copies',
            '1',
            'docker://' + IMAGE + '@' + document['image']['index_digest'],
            'docker://' + IMAGE + ':' + document['version'],
        )
        if image_digest(IMAGE + ':' + document['version']) != document['image']['index_digest']:
            raise ValueError('Promoted tag differs from the accepted index')
        command(
            'gh', 'release', 'edit', document['source']['tag'], '--repo', REPOSITORY, '--draft=false', '--prerelease'
        )
        return fence

    push_metadata(
        options.directory / 'repository',
        'hass-apps',
        text(staging['previous']) if staging['previous'] else '',
        resume=True,
        before_push=publish,
    )


def promote_draft(options: PromoteDraft) -> None:
    from hass_release import verify_provenance

    version, identity = release_guard(ReleaseGuard(source=options.source))
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Promotion scratch already exists')
    state = object_value(
        json.loads(command('gh', 'release', 'view', 'v' + version, '--repo', REPOSITORY, '--json', 'isDraft'))
    )
    if not isinstance(state.get('isDraft'), bool):
        raise ValueError('Promotion requires an existing release with a known draft state')
    options.output.mkdir(parents=True)
    downloaded = options.output / 'downloads'
    downloaded.mkdir()
    command('gh', 'release', 'download', 'v' + version, '--repo', REPOSITORY, '--dir', str(downloaded))
    document = cast(Release, read_json(downloaded / 'release.json'))
    validate_document(document, 'release')
    candidate, bundles = options.output / 'candidate', options.output / 'bundles'
    candidate.mkdir()
    bundles.mkdir()
    payload = {entry['name'] for entry in inventory(document)} | {'release.json', 'SHA256SUMS'}
    signatures = {'artifacts.sigstore.jsonl', 'checksums.sigstore.jsonl', 'acceptance.sigstore.jsonl'} | {
        digest.replace(':', '-') + '.sigstore.jsonl'
        for digest in {document['image']['index_digest'], *(row['manifest_digest'] for row in document['platforms'])}
    }
    check_directory(downloaded, payload | signatures | {'haos-acceptance.tar.gz'})
    for name in payload:
        (downloaded / name).rename(candidate / name)
    for name in signatures:
        (downloaded / name).rename(bundles / name)
    provenance = VerifyProvenance(
        directory=candidate, bundles=bundles, version=version, commit=text(identity['commit'])
    )
    verify_provenance(provenance)
    acceptance = options.output / 'haos-acceptance.tar.gz'
    (downloaded / acceptance.name).rename(acceptance)
    attest(provenance, str(acceptance), 'acceptance.sigstore.jsonl')
    evidence = options.output / 'evidence'
    staging = extract_evidence(acceptance, evidence)
    if not re.fullmatch(r'(?:[0-9a-f]{40})?', str(staging.get('previous'))):
        raise ValueError('Invalid previous store commit in signed staging evidence')
    (options.output / 'staging.json').write_bytes(json_bytes(staging))
    verify_acceptance(evidence, document, staging)
    checked_candidate(options.source, options.output)
    native = options.output / 'native'
    native.mkdir()
    for entry in document['files']:
        if entry['role'] in ('image_archive', 'sbom'):
            os.link(candidate / entry['name'], native / entry['name'])
    (native / 'native-build.json').write_bytes(
        json_bytes(
            {
                'schema_version': 1,
                'version': version,
                'source': document['source'],
                'image_repository': IMAGE,
                'platform': document['platforms'][0],
            }
        )
    )
    repository = Path(
        command(
            'nix',
            'build',
            str(options.source) + '#garmin-hass-app-repository',
            '--no-link',
            '--print-out-paths',
            '--no-update-lock-file',
        )
    )
    copy_repository(repository, options.output / 'repository')
    promote_candidate(
        StagePublication(source=options.source, directory=options.output, build=native), document, staging
    )


def rollback_store(options: RollbackStore) -> None:
    store().rollback(options.directory)


def cleanup_staging(options: CleanupStaging) -> None:
    try:
        rollback_store(options)
    finally:
        cleanup_candidate_branch(options)


def cleanup_candidate_branch(options: CleanupStaging) -> None:
    path = options.directory / 'staging.json'
    if not path.is_file():
        return
    staging = object_value(read_json(path))
    branch, commit = text(staging.get('branch')), text(staging.get('commit'))
    if not re.fullmatch(r'hass-candidate-[1-9][0-9]*-[1-9][0-9]*', branch) or not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('Invalid owned staging branch identity')
    current = branch_head(branch)
    if not current:
        return
    if current != commit:
        raise ValueError('Candidate branch changed; refusing to delete it')
    git('push', '--force-with-lease=refs/heads/' + branch + ':' + commit, REMOTE, ':refs/heads/' + branch)
