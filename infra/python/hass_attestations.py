"""Enforce the release workflow's certificate identity through GitHub's Sigstore verifier."""

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

from hass_release_contract import Release
from json_data import regular_file

REPOSITORY = 'kubijo/garmin-toolkit'
WORKFLOW = REPOSITORY + '/.github/workflows/hass-release.yml'


@dataclass(frozen=True, kw_only=True)
class VerifyProvenance:
    """Verify downloaded bundles, release contents, and digest-addressed registry attestations."""

    directory: Path
    bundles: Path
    version: str
    commit: str


def attest(options: VerifyProvenance, subject: str, bundle: str) -> None:
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', options.version) or not re.fullmatch(
        r'[0-9a-f]{40}', options.commit
    ):
        raise ValueError('Expected an independently trusted release version and full source commit')
    path = options.bundles / bundle
    regular_file(path)
    if path.stat().st_size > 8 * 1024 * 1024:
        raise ValueError('Attestation bundle exceeds the size limit')
    reference = 'refs/tags/v' + options.version
    subprocess.run(
        [
            'gh',
            'attestation',
            'verify',
            subject,
            '--bundle',
            str(path),
            '--hostname',
            'github.com',
            '--repo',
            REPOSITORY,
            '--signer-workflow',
            WORKFLOW,
            '--cert-identity',
            'https://github.com/' + WORKFLOW + '@' + reference,
            '--cert-oidc-issuer',
            'https://token.actions.githubusercontent.com',
            '--source-ref',
            reference,
            '--source-digest',
            options.commit,
            '--signer-digest',
            options.commit,
            '--deny-self-hosted-runners',
            '--predicate-type',
            'https://slsa.dev/provenance/v1',
            '--format',
            'json',
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=120,
    )


def attest_payloads(options: VerifyProvenance, document: Release) -> None:
    for entry in document['files']:
        attest(options, str(options.directory / entry['name']), 'artifacts.sigstore.jsonl')
    for digest in sorted(
        {document['image']['index_digest'], *(item['manifest_digest'] for item in document['platforms'])}
    ):
        attest(
            options,
            'oci://' + document['image']['repository'] + '@' + digest,
            digest.replace(':', '-') + '.sigstore.jsonl',
        )
