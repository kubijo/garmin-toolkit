#!/usr/bin/env python3
"""Generate distributable third-party license bundles."""

import filecmp
import json
import shlex
import subprocess
import sys
import tempfile
import tomllib
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import tyro

from declarations import declaration_supplements, standard_terms, verified_text

REPOSITORY = Path.cwd()
DEFAULT_OUTPUT = REPOSITORY / 'assets' / 'licenses'
NOTICE_MANIFEST = Path(__file__).with_name('notices.toml')


@dataclass(frozen=True, kw_only=True)
class Options:
    """Generate or check distributable third-party license bundles."""

    config: Path
    check: bool = False
    output: Path = DEFAULT_OUTPUT


def run_json(arguments: list[str]) -> dict[str, Any]:
    try:
        result = subprocess.run(
            arguments,
            cwd=REPOSITORY,
            check=True,
            capture_output=True,
            text=True,
        )
    except subprocess.CalledProcessError as error:
        sys.stderr.write(error.stderr or '')
        raise
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
    metadata = run_json(['cargo', 'metadata', '--format-version', '1', '--locked'])
    raw = run_json(['cargo', 'bundle-licenses', '--format', 'json', '--output', '-'])
    entries: dict[tuple[str, str], dict[str, Any]] = {
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
    packages = {(package['name'], package['version']): package for package in metadata['packages']}
    supplements = notice_supplements(packages)
    expected = {(*key, notice['license']) for key, entry in entries.items() for notice in entry['notices']}
    if unused := supplements.keys() - expected:
        raise ValueError(f'stale license supplements: {sorted(unused)}')
    for key, entry in entries.items():
        package = packages[key]
        local_texts = package_notices(package)
        for notice in entry['notices']:
            supplement = supplements.get((*key, notice['license']))
            if notice['text'] in local_texts and (supplement is None or not supplement.startswith(notice['text'])):
                continue
            if supplement is not None:
                notice['text'] = supplement
            else:
                # The upstream harvester climbs ancestors, including our checkout
                # when CARGO_HOME lives here. Never trust text outside this package.
                notice['text'] = ''
    return entries


def package_file(package: dict[str, Any], relative: str) -> Path:
    root = Path(package['manifest_path']).resolve().parent
    path = (root / relative).resolve()
    if not path.is_relative_to(root):
        raise ValueError(f'license path escapes {package["name"]}: {relative}')
    return path


def package_notices(package: dict[str, Any]) -> set[str]:
    """Bound accepted discovery to notice files shipped inside the package."""
    root = Path(package['manifest_path']).resolve().parent
    files: list[Path] = []
    for path in sorted(root.iterdir()):
        if not path.resolve().is_relative_to(root):
            continue
        if path.name.lower().startswith(('license', 'licence', 'copying', 'copyright', 'notice', 'unlicense')):
            files.extend(path.rglob('*') if path.is_dir() else [path])
    if package.get('license_file'):
        files.append(package_file(package, package['license_file']))
    texts: set[str] = set()
    for path in files:
        if path.is_file() and path.resolve().is_relative_to(root):
            texts.add(path.read_bytes().decode('utf-8'))
    return texts


def notice_supplements(
    packages: dict[tuple[str, str], dict[str, Any]], manifest: Path = NOTICE_MANIFEST
) -> dict[tuple[str, str, str], str]:
    """Load version-specific package paths or checked-in, pinned upstream notices."""
    configured = tomllib.loads(manifest.read_text())
    supplements = declaration_supplements(packages, configured, manifest.parent)
    for notice in configured['notices']:
        for identifier in notice['packages']:
            name, version = identifier.split(' ', 1)
            key = (name, version)
            if key not in packages:
                raise ValueError(f'stale license supplement: {identifier}')
            if 'package_file' in notice:
                text = package_file(packages[key], notice['package_file']).read_bytes().decode('utf-8')
            else:
                text = verified_text(manifest.parent, notice)
            if notice.get('include_standard_terms', False):
                text += '\n\n' + standard_terms(notice['license'], configured, manifest.parent)
            full_key = (*key, notice['license'])
            if full_key in supplements:
                raise ValueError(f'duplicate license supplement: {full_key}')
            supplements[full_key] = text
    return supplements


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
    missing: list[str] = []
    for entry in ordered:
        absent = [
            notice['license']
            for notice in entry['notices']
            if not notice['text'].strip() or notice['text'].strip() == 'NOT FOUND'
        ]
        if absent or not entry['notices']:
            missing.append(f'{entry["name"]} {entry["version"]} ({", ".join(absent) or "no notices"})')
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
    documents: dict[str, str] = {}
    for target in config['targets']:
        linked: dict[tuple[str, str], bool] = {}
        for component in [target, *target.get('bundled', [])]:
            for triple in component['triples']:
                linked.update(resolve_linked(cargo_metadata(triple), component['package']))

        external_missing = sorted(key for key, external in linked.items() if external and key not in cargo)
        if external_missing:
            raise ValueError(f'cargo-bundle-licenses omitted {external_missing}')

        entries = [cargo[key] for key in sorted(linked) if key in cargo]
        entries.extend(
            asset
            for asset, configured in zip(assets, config['assets'], strict=True)
            if target['name'] in configured['targets']
        )
        documents[f'bundle-{target["name"]}.json'] = bundle_document(entries)

    # Validate every target before updating any distributable bundle.
    output.mkdir(parents=True, exist_ok=True)
    for stale in output.glob('bundle-*.json'):
        if stale.name not in documents:
            stale.unlink()
    for name, document in documents.items():
        (output / name).write_text(document)


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
            for name in sorted(stale & expected & present):
                before = expanded_entries(output / name)
                after = expanded_entries(fresh / name)
                changed = sorted(key for key in before.keys() | after.keys() if before.get(key) != after.get(key))
                print(f'  {name}: changed packages: {", ".join(changed) or "bundle encoding"}')
            return False
    return True


def expanded_entries(path: Path) -> dict[str, Any]:
    """Compare actual notices rather than shifted indices into the shared text table."""
    bundle = json.loads(path.read_text())
    return {
        f'{entry["name"]} {entry["version"]}': {
            **entry,
            'notices': [
                {
                    'license': notice['license'],
                    'text': bundle['license_texts'][notice['text_index']],
                }
                for notice in entry['notices']
            ],
        }
        for entry in bundle['entries']
    }


def main() -> int:
    arguments = tyro.cli(Options)
    config = json.loads(arguments.config.read_text())
    try:
        if arguments.check:
            return 0 if check(config, arguments.output) else 1
        generate(config, arguments.output)
    except subprocess.CalledProcessError as error:
        print(f'license generation failed: {shlex.join(error.cmd)} (exit {error.returncode})', file=sys.stderr)
        return 1
    except (OSError, ValueError) as error:
        print(f'license generation failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
