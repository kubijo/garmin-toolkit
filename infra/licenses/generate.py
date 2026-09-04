#!/usr/bin/env python3
"""Generate distributable third-party license bundles."""

import argparse
import filecmp
import json
import subprocess
import tempfile
from collections import deque
from pathlib import Path
from typing import Any

REPOSITORY = Path.cwd()
DEFAULT_OUTPUT = REPOSITORY / 'assets' / 'licenses'


def run_json(arguments: list[str]) -> dict[str, Any]:
    result = subprocess.run(
        arguments,
        cwd=REPOSITORY,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def cargo_metadata(triple: str) -> dict[str, Any]:
    return run_json(
        [
            'cargo',
            'metadata',
            '--format-version',
            '1',
            '--locked',
            '--filter-platform',
            triple,
        ]
    )


def resolve_linked(metadata: dict[str, Any], package_name: str) -> dict[tuple[str, str], bool]:
    packages = {package['id']: package for package in metadata['packages']}
    roots = [
        package['id']
        for package in metadata['packages']
        if package['name'] == package_name and package['source'] is None
    ]
    if len(roots) != 1:
        raise ValueError(f'expected one workspace package named `{package_name}`')

    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    linked: set[str] = set()
    pending = deque(roots)
    while pending:
        package_id = pending.popleft()
        if package_id in linked:
            continue
        linked.add(package_id)
        for dependency in nodes[package_id]['deps']:
            normal = any(kind['kind'] in (None, 'normal') for kind in dependency['dep_kinds'])
            package = packages[dependency['pkg']]
            proc_macro = any('proc-macro' in target['kind'] for target in package['targets'])
            if normal and not proc_macro:
                pending.append(dependency['pkg'])

    return {(packages[key]['name'], packages[key]['version']): packages[key]['source'] is not None for key in linked}


def harvested_entries() -> dict[tuple[str, str], dict[str, Any]]:
    raw = run_json(['cargo', 'bundle-licenses', '--format', 'json', '--output', '-'])
    return {
        (library['package_name'], library['package_version']): {
            'license': library['license'],
            'name': library['package_name'],
            'notices': [
                {
                    'license': found['license'],
                    'text': found['text'] or '',
                }
                for found in library['licenses']
            ],
            'source': {'kind': 'cargo'},
            'version': library['package_version'],
        }
        for library in raw['third_party_libraries']
    }


def asset_entry(asset: dict[str, Any]) -> dict[str, Any]:
    text = Path(asset['license_file']).read_text()
    return {
        'license': asset['license'],
        'name': asset['name'],
        'notices': [{'license': asset['license'], 'text': text}],
        'source': {'kind': 'asset', 'url': asset['source']},
        'version': asset['version'],
    }


def bundle_document(entries: list[dict[str, Any]]) -> str:
    ordered = sorted(entries, key=lambda entry: (entry['name'], entry['version']))
    missing = [
        f'{entry["name"]} {entry["version"]}'
        for entry in ordered
        if not any(notice['text'] for notice in entry['notices'])
    ]
    if missing:
        raise ValueError(f'missing license text for {missing}')
    texts = sorted({notice['text'] for entry in ordered for notice in entry['notices'] if notice['text']})
    indices = {text: index for index, text in enumerate(texts)}
    encoded = [
        {
            'name': entry['name'],
            'version': entry['version'],
            'license': entry['license'],
            'source': entry['source'],
            'notices': [
                {
                    'license': notice['license'],
                    'text_index': indices[notice['text']],
                }
                for notice in entry['notices']
                if notice['text']
            ],
        }
        for entry in ordered
    ]
    return (
        json.dumps(
            {'schema_version': 1, 'entries': encoded, 'license_texts': texts},
            indent=2,
        )
        + '\n'
    )


def generate(config: dict[str, Any], output: Path) -> None:
    cargo = harvested_entries()
    assets = [asset_entry(asset) for asset in config['assets']]
    output.mkdir(parents=True, exist_ok=True)
    expected = {f'bundle-{target["name"]}.json' for target in config['targets']}
    for stale in output.glob('bundle-*.json'):
        if stale.name not in expected:
            stale.unlink()

    for target in config['targets']:
        linked: dict[tuple[str, str], bool] = {}
        for triple in target['triples']:
            linked.update(resolve_linked(cargo_metadata(triple), target['package']))

        external_missing = sorted(key for key, external in linked.items() if external and key not in cargo)
        if external_missing:
            raise ValueError(f'cargo-bundle-licenses omitted {external_missing}')

        entries = [cargo[key] for key in sorted(linked) if key in cargo]
        entries.extend(
            asset
            for asset, configured in zip(assets, config['assets'], strict=True)
            if target['name'] in configured['targets']
        )
        path = output / f'bundle-{target["name"]}.json'
        path.write_text(bundle_document(entries))


def check(config: dict[str, Any], output: Path) -> bool:
    with tempfile.TemporaryDirectory() as directory:
        fresh = Path(directory)
        generate(config, fresh)
        expected = {f'bundle-{target["name"]}.json' for target in config['targets']}
        present = {path.name for path in output.glob('bundle-*.json')}
        stale = expected.symmetric_difference(present)
        stale |= {name for name in expected & present if not filecmp.cmp(output / name, fresh / name, shallow=False)}
        if stale:
            print(f'license bundles are stale: {", ".join(sorted(stale))}')
            return False
    return True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=DEFAULT_OUTPUT)
    arguments = parser.parse_args()
    config = json.loads(arguments.config.read_text())
    if arguments.check:
        return 0 if check(config, arguments.output) else 1
    generate(config, arguments.output)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
