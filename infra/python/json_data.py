"""Narrow decoded JSON containers; consumers validate their individual field schemas."""

import gzip
import hashlib
import json
import stat
from pathlib import Path
from typing import Any, TypeGuard, cast

type Object = dict[str, object]
MAX_JSON_BYTES = 8 * 1024 * 1024


def unique_object(pairs: list[tuple[str, object]]) -> Object:
    """Reject ambiguous duplicate fields when decoding integrity metadata."""
    result: Object = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'Duplicate JSON key: {key}')
        result[key] = value
    return result


def object_value(value: object) -> Object:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in cast(dict[object, object], value)):
        raise ValueError('Expected a JSON object with string keys')
    return cast(Object, value)


def object_rows(value: object) -> list[Object]:
    if not isinstance(value, list):
        raise ValueError('Expected an array of JSON objects')
    return [object_value(row) for row in cast(list[object], value)]


def text(value: object) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError('Expected a nonempty string')
    return value


def strings(value: object) -> list[str]:
    if not isinstance(value, list):
        raise ValueError('Expected an array of strings')
    return [text(item) for item in cast(list[object], value)]


def is_object(value: object) -> TypeGuard[dict[str, Any]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in cast(dict[object, object], value))


def is_array(value: object) -> TypeGuard[list[Any]]:
    return isinstance(value, list)


def regular_file(path: Path) -> None:
    """Reject links and special files before opening or hashing a release payload."""
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
        raise ValueError(f'Expected a nonempty regular file: {path}')


def file_sha256(path: Path) -> str:
    regular_file(path)
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def read_json(path: Path, *, max_bytes: int = MAX_JSON_BYTES) -> object:
    regular_file(path)
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rb') as stream:
        data = stream.read(max_bytes + 1)
    if len(data) > max_bytes:
        raise ValueError(f'JSON metadata exceeds {max_bytes} bytes: {path}')
    return json.loads(data, object_pairs_hook=unique_object)


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False) + '\n').encode()
