"""Verified package declarations paired with pinned standard license terms."""

import hashlib
import json
import re
import tomllib
from pathlib import Path
from typing import Any


def verified_text(directory: Path, source: dict[str, Any]) -> str:
    """Check stored source bytes before applying any declared template adjustment."""
    if not re.fullmatch(r'https://[^\s]+/[a-f0-9]{40}/[^\s]+', source['source']):
        raise ValueError(f'upstream license source must pin a revision: {source["source"]}')
    path = (directory / source['file']).resolve()
    if not path.is_relative_to(directory.resolve()):
        raise ValueError(f'license source escapes manifest directory: {source["file"]}')
    data = path.read_bytes()
    if source.get('no_final_newline', False):
        if not data.endswith(b'\n'):
            raise ValueError(f'stored upstream notice requires a final newline: {source["file"]}')
        data = data[:-1]
    if hashlib.sha256(data).hexdigest() != source['sha256']:
        raise ValueError(f'upstream license checksum mismatch: {source["file"]}')
    return data.decode('utf-8')


def verified_declaration(package: dict[str, Any], declaration: dict[str, Any]) -> None:
    """Bind a reviewed declaration to the published manifest and its VCS origin."""
    root = Path(package['manifest_path']).resolve().parent
    files = [root / 'Cargo.toml.orig', root / '.cargo_vcs_info.json']
    if any(not path.resolve().is_relative_to(root) for path in files):
        raise ValueError(f'license declaration escapes package: {declaration["package"]}')
    original = files[0].read_bytes()
    if hashlib.sha256(original).hexdigest() != declaration['sha256']:
        raise ValueError(f'license declaration checksum mismatch: {declaration["package"]}')
    published = tomllib.loads(original.decode('utf-8'))['package']
    for field in ('name', 'version', 'license', 'authors'):
        if published[field] != package[field]:
            raise ValueError(f'license declaration metadata mismatch: {declaration["package"]} ({field})')
    if package['license'] != declaration['expression']:
        raise ValueError(f'license expression changed: {declaration["package"]}')

    vcs = json.loads(files[1].read_text())
    repository = package['repository'].replace('http://', 'https://').removesuffix('.git')
    revision = vcs['git']['sha1']
    if not re.fullmatch(r'[a-f0-9]{40}', revision):
        raise ValueError(f'invalid package revision: {declaration["package"]}')
    relative = '/'.join(part for part in (vcs.get('path_in_vcs', ''), 'Cargo.toml') if part)
    if repository.startswith('https://github.com/'):
        source = repository.replace('https://github.com/', 'https://raw.githubusercontent.com/')
        expected = f'{source}/{revision}/{relative}'
    elif repository.startswith('https://gitlab.com/'):
        expected = f'{repository}/-/raw/{revision}/{relative}'
    else:
        raise ValueError(f'unsupported declaration repository: {repository}')
    if declaration['source'] != expected:
        raise ValueError(f'license declaration origin mismatch: {declaration["package"]}')


def standard_terms(license_id: str, config: dict[str, Any], directory: Path) -> str:
    """Include full, pinned standard terms while omitting only copyright examples."""
    source = config['terms'].get(license_id)
    if source is None:
        raise ValueError(f'unreviewed standard license terms: {license_id}')
    expected_source = (
        r'https://raw\.githubusercontent\.com/spdx/license-list-data/'
        + r'[a-f0-9]{40}/text/'
        + re.escape(license_id)
        + r'\.txt'
    )
    if not re.fullmatch(expected_source, source['source']):
        raise ValueError(f'SPDX terms source does not match license: {license_id}')
    text = verified_text(directory, source)
    if placeholder := source.get('omit_template_line'):
        if not re.fullmatch(r'Copyright \(c\) <year> <[a-z ]+>\.?', placeholder):
            raise ValueError(f'only copyright examples may be omitted: {license_id}')
        lines = text.splitlines(keepends=True)
        if sum(line.strip() == placeholder for line in lines) != 1:
            raise ValueError(f'license template line changed: {license_id}')
        text = ''.join(line for line in lines if line.strip() != placeholder).lstrip('\n')
    return f'Standard license terms ({license_id}): {source["source"]}\n\n{text}'


def declaration_supplements(
    packages: dict[tuple[str, str], dict[str, Any]], config: dict[str, Any], directory: Path
) -> dict[tuple[str, str, str], str]:
    """Use reviewed SPDX declarations when a package supplies no separate notice."""
    supplements: dict[tuple[str, str, str], str] = {}
    for declaration in config.get('declarations', []):
        name, version = declaration['package'].split(' ', 1)
        key = (name, version)
        if key not in packages:
            raise ValueError(f'stale license declaration: {declaration["package"]}')
        package = packages[key]
        verified_declaration(package, declaration)
        # These reviewed declarations are single licenses or OR alternatives.
        # Other expressions require their own review; never flatten AND or WITH.
        licenses = declaration['expression'].split(' OR ')
        if licenses != declaration['licenses']:
            raise ValueError(f'license alternatives mismatch: {declaration["package"]}')
        for license_id in licenses:
            # Author metadata is attributed as such. No copyright owner or year
            # is inferred from author names or the standard template's examples.
            attribution = '; '.join(package['authors'])
            header = (
                f'Package: {declaration["package"]}\n'
                f'Authors (Cargo metadata): {attribution}\n'
                f'Declared license: {declaration["expression"]}\n'
                f'Declaration: {declaration["source"]}\n'
            )
            full_key = (*key, license_id)
            if full_key in supplements:
                raise ValueError(f'duplicate license declaration: {full_key}')
            supplements[full_key] = header + standard_terms(license_id, config, directory)
    return supplements
