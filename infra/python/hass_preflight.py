"""Exercise branch publication using native receipts, without admitting a tagged release."""

import os
import re
import shutil
import tempfile
from pathlib import Path
from typing import cast

from hass_attestations import WORKFLOW, attest_reference
from hass_publish import (
    IMAGE,
    REMOTE,
    PreparePublication,
    ReleaseEvidence,
    acceptance_directory,
    archive_evidence,
    build_directories,
    check_acceptance,
    check_platforms,
    copy_platform_files,
    extract_evidence,
    image_manifest,
    require_rejection,
    source_parts,
)
from hass_registry import PrepareRegistry, prepare_registry
from hass_release import check_payload_metadata, copy_payload
from hass_release_build import build_source
from hass_release_contract import (
    Payload,
    check_directory,
    check_shape,
    describe_file,
    environment,
    inventory,
    validate_document,
)
from hass_repository import copy_repository
from json_data import Object, file_sha256, json_bytes, object_rows, object_value, read_json, regular_file, text


def guard(source: Path) -> tuple[str, Object]:
    version, identity = build_source(source)
    reference = os.environ.get('GITHUB_REF', '')
    if (
        os.environ.get('GITHUB_ACTIONS') != 'true'
        or os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch'
        or os.environ.get('GITHUB_REPOSITORY') != 'kubijo/garmin-toolkit'
        or not re.fullmatch(r'refs/heads/[^\s]+', reference)
        or os.environ.get('GITHUB_SHA') != identity['commit']
        or os.environ.get('GITHUB_WORKFLOW_REF') != WORKFLOW + '@' + reference
    ):
        raise ValueError('Preflight requires a manual branch run of the canonical release workflow at its exact commit')
    return version, identity


def payload(document: Object, sources: Path, *, candidate: bool = False) -> Payload:
    validate_document(document, 'registry')
    if document['registry_reference'] is not None or document['acceptance'] != []:
        raise ValueError('Preflight requires freshly prepared native images')
    result = cast(Payload, {key: document[key] for key in ('version', 'image', 'platforms')})
    check_platforms(result['platforms'])
    result['source_parts'] = source_parts(result, sources, candidate=candidate)
    return result


def checksum_bytes(candidate: Path, document: Payload) -> bytes:
    files = [describe_file(candidate, spec) for spec in inventory(document)]
    files.append(describe_file(candidate, {'name': 'registry.json', 'role': 'registry'}))
    return environment().get_template('SHA256SUMS.j2').render(files=sorted(files, key=lambda row: row['name'])).encode()


def prepare(options: PreparePublication) -> None:
    version, identity = guard(options.source)
    check_shape(options.repository, 'hass-app-repository.dirspec.toml')
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Preflight output already exists')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-preflight.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'publication'
        staged.mkdir()
        document = prepare_registry(
            PrepareRegistry(source=options.source, builds=build_directories(options.build), output=staged / 'registry')
        )
        if document['source'] != identity or document['version'] != version:
            raise ValueError('Native preflight receipt differs from the dispatched source')
        inputs = payload(document, options.sources)
        artifacts, candidate = staged / 'artifacts', staged / 'candidate'
        copy_platform_files(options.build, options.sources, inputs, artifacts)
        candidate.mkdir()
        copy_payload(options.source, text(identity['commit']), inputs, artifacts, candidate)
        shutil.rmtree(artifacts)
        (candidate / 'registry.json').write_bytes(json_bytes(document))
        (candidate / 'SHA256SUMS').write_bytes(checksum_bytes(candidate, inputs))
        check_candidate(options.source, candidate)
        copy_repository(
            options.repository,
            staged / 'repository',
            architectures=tuple(platform['architecture'] for platform in inputs['platforms']),
        )
        staged.rename(options.output)


def check_candidate(source: Path, candidate: Path, authenticated: bytes | None = None) -> tuple[Object, Payload]:
    version, identity = guard(source)
    document = object_value(read_json(candidate / 'registry.json'))
    inputs = payload(document, candidate, candidate=True)
    if document['version'] != version or document['source'] != identity:
        raise ValueError('Preflight inventory differs from the dispatched source')
    check_directory(candidate, {spec['name'] for spec in inventory(inputs)} | {'registry.json', 'SHA256SUMS'})
    check_shape(candidate, 'hass-preflight.dirspec.toml')
    expected = (candidate / 'SHA256SUMS').read_bytes() if authenticated is None else authenticated
    if expected != checksum_bytes(candidate, inputs):
        raise ValueError('Preflight checksums differ from the complete native inventory')
    check_payload_metadata(candidate, inputs, require_resolved=True)
    return document, inputs


def package(options: ReleaseEvidence) -> None:
    document, inputs = check_candidate(options.source, options.directory / 'candidate')
    for platform in inputs['platforms']:
        check_acceptance(
            acceptance_directory(options.evidence, inputs['platforms'], platform),
            platform,
            document['source'],
            inputs['image']['index_digest'],
            object_value(read_json(options.directory / 'staging.json')),
            release_version=inputs['version'],
            version=inputs['version'] if len(inputs['platforms']) > 1 else None,
        )
    archive_evidence(options.directory, options.evidence, inputs['platforms'])


def verify(source: Path, directory: Path, bundles: Path) -> None:
    _, identity = guard(source)
    reference, commit = os.environ['GITHUB_REF'], text(identity['commit'])
    candidate = directory / 'candidate'
    sums = candidate / 'SHA256SUMS'
    regular_file(sums)
    with sums.open('rb') as stream:
        authenticated = stream.read(1024 * 1024 + 1)
    if len(authenticated) > 1024 * 1024:
        raise ValueError('Preflight checksums exceed the size limit')
    with tempfile.TemporaryDirectory(prefix='.preflight-verification.', dir=directory.parent) as temporary:
        scratch = Path(temporary)
        snapshot = scratch / 'SHA256SUMS'
        snapshot.write_bytes(authenticated)
        attest_reference(str(snapshot), bundles / 'checksums.sigstore.jsonl', reference, commit)
        hashes = re.findall(rb'^([0-9a-f]{64})  registry\.json$', authenticated, re.MULTILINE)
        if len(hashes) != 1 or file_sha256(candidate / 'registry.json') != hashes[0].decode():
            raise ValueError('Authenticated checksums must bind the native registry receipt')
        document, inputs = check_candidate(source, candidate, authenticated)
        if sums.read_bytes() != authenticated:
            raise ValueError('Authenticated preflight inventory changed during verification')
        digests = {inputs['image']['index_digest'], *(platform['manifest_digest'] for platform in inputs['platforms'])}
        check_directory(
            bundles,
            {'artifacts.sigstore.jsonl', 'checksums.sigstore.jsonl', 'acceptance.sigstore.jsonl'}
            | {digest.replace(':', '-') + '.sigstore.jsonl' for digest in digests},
        )
        for name in [*(spec['name'] for spec in inventory(inputs)), 'registry.json']:
            attest_reference(str(candidate / name), bundles / 'artifacts.sigstore.jsonl', reference, commit)
        for digest in sorted(digests):
            attest_reference(
                'oci://' + IMAGE + '@' + digest,
                bundles / (digest.replace(':', '-') + '.sigstore.jsonl'),
                reference,
                commit,
            )
        acceptance = directory / 'haos-acceptance.tar.gz'
        attest_reference(str(acceptance), bundles / 'acceptance.sigstore.jsonl', reference, commit)
        evidence = scratch / 'evidence'
        staging = extract_evidence(acceptance, evidence, inputs['platforms'])
        for platform in inputs['platforms']:
            check_acceptance(
                acceptance_directory(evidence, inputs['platforms'], platform),
                platform,
                document['source'],
                inputs['image']['index_digest'],
                staging,
                release_version=inputs['version'],
                version=inputs['version'] if len(inputs['platforms']) > 1 else None,
            )
        branch = text(staging.get('branch'))
        if (
            not re.fullmatch(r'hass-candidate-[1-9][0-9]*-[1-9][0-9]*', branch)
            or staging.get('repository_url') != REMOTE + '#' + branch
            or staging.get('index_digest') != inputs['image']['index_digest']
        ):
            raise ValueError('Preflight acceptance must identify the staged repository')
    (directory / 'preflight-verification.json').write_bytes(
        json_bytes(
            {
                'passed': True,
                'reference': reference,
                'source_commit': commit,
                'index_digest': inputs['image']['index_digest'],
            }
        )
    )


def qualify_rejections(source: Path, directory: Path, bundles: Path) -> None:
    _, identity = guard(source)
    reference, commit = os.environ['GITHUB_REF'], text(identity['commit'])
    checksums = str(directory / 'candidate/SHA256SUMS')
    wrong_commit = ('0' if commit[0] != '0' else '1') + commit[1:]
    require_rejection(
        lambda: attest_reference(checksums, bundles / 'checksums.sigstore.jsonl', reference, wrong_commit)
    )
    require_rejection(
        lambda: attest_reference(checksums, bundles / 'checksums.sigstore.jsonl', reference + '-untrusted', commit)
    )
    document = object_value(read_json(directory / 'candidate/registry.json'))
    digest = text(object_value(document['image'])['index_digest'])
    manifest = text(object_rows(document['platforms'])[0]['manifest_digest'])
    with tempfile.TemporaryDirectory(prefix='.preflight-rejection.', dir=directory.parent) as temporary:
        altered = Path(temporary) / 'altered-payload'
        altered.write_text('Deliberately altered preflight payload\n')
        require_rejection(
            lambda: attest_reference(str(altered), bundles / 'artifacts.sigstore.jsonl', reference, commit)
        )
        subject = Path(temporary) / 'index.json'
        subject.write_bytes(image_manifest(IMAGE + '@' + digest))
        if file_sha256(subject) != digest.removeprefix('sha256:') or digest == manifest:
            raise ValueError('Wrong-subject qualification requires the unchanged distinct OCI index')
        attest_reference(str(subject), bundles / (digest.replace(':', '-') + '.sigstore.jsonl'), reference, commit)
        require_rejection(
            lambda: attest_reference(
                str(subject), bundles / (manifest.replace(':', '-') + '.sigstore.jsonl'), reference, commit
            )
        )
    (directory / 'provenance-qualification.json').write_bytes(
        json_bytes(
            {
                'passed': True,
                'reference': reference,
                'source_commit': commit,
                'index_digest': digest,
                'rejections': ['wrong-commit', 'wrong-signer', 'altered-payload', 'wrong-platform-subject'],
            }
        )
    )
