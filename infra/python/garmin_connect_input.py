"""Update the byte-exact Garmin Personal API input from a reviewed upstream commit."""

import argparse
import hashlib
import json
import re
import urllib.request
from pathlib import Path

from json_data import object_value, unique_object

ROOT = Path(__file__).resolve().parents[2]
DIRECTORY = ROOT / 'infra/garmin-connect'
REPOSITORY = 'https://github.com/Likenttt/connect-personal-api-ts'
MAX_BYTES = 8 * 1024 * 1024


def verify(data: bytes, digest: str) -> None:
    """Reject mismatched artifacts and missing operations before replacing the input."""
    if not re.fullmatch(r'[a-f0-9]{64}', digest) or hashlib.sha256(data).hexdigest() != digest:
        raise ValueError('Descriptor SHA-256 mismatch')
    spec = object_value(json.loads(data, object_pairs_hook=unique_object))
    info = object_value(spec.get('info'))
    if spec.get('openapi') != '3.0.1' or info.get('title') != 'Garmin Connect Personal API':
        raise ValueError('Unexpected API descriptor')
    paths = object_value(spec.get('paths'))
    for path in (
        '/userprofile-service/userprofile/settings',
        '/activity-service/activity/{activityId}',
        '/activitylist-service/activities/search/activities',
        '/download-service/files/activity/{activityId}',
    ):
        operation = object_value(object_value(paths.get(path)).get('get'))
        responses = object_value(operation.get('responses'))
        if not responses:
            raise ValueError(f'Missing response contract: {path}')
    object_value(object_value(spec.get('components')).get('schemas'))


def update(revision: str, digest: str) -> None:
    if not re.fullmatch(r'[a-f0-9]{40}', revision):
        raise ValueError('Use a full upstream commit hash')
    url = f'https://raw.githubusercontent.com/Likenttt/connect-personal-api-ts/{revision}/spec/openapi.json'
    with urllib.request.urlopen(url, timeout=30) as response:
        data = response.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise ValueError('Descriptor exceeds 8 MiB')
    verify(data, digest)
    pin = {'repository': REPOSITORY, 'revision': revision, 'sha256': digest}
    (DIRECTORY / 'openapi.json').write_bytes(data)
    (DIRECTORY / 'input.json').write_text(json.dumps(pin, indent=2) + '\n')
    print('Updated descriptor and pin; run just connect-api::test before accepting the diff.')


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('revision', help='Reviewed upstream commit, 40 lowercase hex characters')
    parser.add_argument('sha256', help='Expected digest of the raw descriptor')
    args = parser.parse_args()
    update(args.revision, args.sha256)


if __name__ == '__main__':
    main()
