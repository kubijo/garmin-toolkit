"""Check Lokalise metadata that FormatJS does not verify."""

import json
import sys
from pathlib import Path
from typing import Any


def metadata_errors(source_catalog: dict[str, Any], translation_catalog: dict[str, Any]) -> list[str]:
    """Return source or description mismatches for shared catalog keys."""
    errors: list[str] = []
    for key in sorted(source_catalog.keys() & translation_catalog.keys()):
        source = source_catalog[key]
        translation = translation_catalog[key]
        if not isinstance(source, dict) or not isinstance(translation, dict):
            errors.append(f'{key}: catalog entry is not an object')
            continue

        expected_source = source.get('message')
        recorded_source = translation.get('source')
        if recorded_source != expected_source:
            errors.append(f'{key}: recorded source {recorded_source!r}; expected {expected_source!r}')

        expected_description = source.get('description')
        recorded_description = translation.get('description')
        if recorded_description != expected_description:
            errors.append(f'{key}: recorded description {recorded_description!r}; expected {expected_description!r}')

        translated = translation.get('translation')
        if not isinstance(translated, str) or not translated.strip():
            errors.append(f'{key}: translation must be a non-blank string')
    return errors


def read_catalog(path: Path) -> dict[str, Any]:
    """Read one JSON object catalog."""
    value = json.loads(path.read_text(encoding='utf-8'))
    if not isinstance(value, dict):
        raise TypeError(f'{path}: catalog root is not an object')
    return value


def main(arguments: list[str]) -> int:
    """Run the metadata check."""
    if len(arguments) != 2:
        print(
            'usage: check_translation_metadata.py SOURCE TRANSLATIONS',
            file=sys.stderr,
        )
        return 2

    try:
        source = read_catalog(Path(arguments[0]))
        translations = read_catalog(Path(arguments[1]))
    except (OSError, TypeError, ValueError) as error:
        print(f'translation metadata check failed: {error}', file=sys.stderr)
        return 1

    errors = metadata_errors(source, translations)
    if not errors:
        return 0

    print('Czech translation catalog validation failed:', file=sys.stderr)
    for error in errors:
        print(f'  - {error}', file=sys.stderr)
    print('Run just dev::i18n-sync and review the resulting entries.', file=sys.stderr)
    return 1


if __name__ == '__main__':
    raise SystemExit(main(sys.argv[1:]))
