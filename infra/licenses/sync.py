"""Extract current upstream notices and synchronize their package versions and source hashes."""

import copy
import hashlib
import json
import re
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NotRequired, TypedDict, cast

import tomlkit

from declarations import verified_declaration, verified_text


class LicenseNotice(TypedDict):
    license: str
    packages: list[str]
    package_file: NotRequired[str]
    file: NotRequired[str]
    source: NotRequired[str]
    sha256: NotRequired[str]
    no_final_newline: NotRequired[bool]
    include_standard_terms: NotRequired[bool]


class LicenseDeclaration(TypedDict):
    package: str
    expression: str
    licenses: list[str]
    source: str
    sha256: str


class ManifestRecords(TypedDict):
    notices: list[LicenseNotice]
    declarations: NotRequired[list[LicenseDeclaration]]


@dataclass(frozen=True)
class NoticeCandidate:
    notice: LicenseNotice
    source: str | None
    matches_package: bool
    matches_revision: bool

    def priority(self) -> tuple[bool, bool]:
        return not self.matches_package, not self.matches_revision


def source_for(package: dict[str, Any], source: str) -> str | None:
    """Keep the reviewed repository-relative license path, replacing only its revision."""
    repository = (package.get('repository') or '').replace('http://', 'https://').removesuffix('.git')
    if repository.startswith('https://github.com/'):
        prefix = repository.replace('https://github.com/', 'https://raw.githubusercontent.com/') + '/'
    elif repository.startswith('https://gitlab.com/'):
        prefix = repository + '/-/raw/'
    else:
        return None
    if not source.startswith(prefix):
        return None
    relative = source[len(prefix) :].split('/', 1)
    if len(relative) != 2 or not re.fullmatch('[a-f0-9]{40}', relative[0]):
        return None
    root = Path(package['manifest_path']).resolve().parent
    vcs_path = root / '.cargo_vcs_info.json'
    if not vcs_path.resolve().is_relative_to(root):
        raise ValueError('Package VCS metadata escapes its source directory')
    revision = json.loads(vcs_path.read_text())['git']['sha1']
    if not re.fullmatch('[a-f0-9]{40}', revision):
        raise ValueError('Invalid published source revision')
    return prefix + revision + '/' + relative[1]


def fetch_notice(source: str) -> bytes:
    """Fetch a pinned notice with a bounded deadline and size."""
    with urllib.request.urlopen(source, timeout=15) as response:
        data = response.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError('Upstream notice exceeds one MiB')
    return data


def synchronize(packages: dict[tuple[str, str], dict[str, Any]], manifest: Path) -> None:
    """Refresh version pins and add sibling crates sharing verified upstream notices."""
    document = tomlkit.parse(manifest.read_text())
    configured = cast(ManifestRecords, document.unwrap())
    originals = configured['notices']
    current = {f'{name} {version}': package for (name, version), package in packages.items()}
    notices: list[LicenseNotice] = []
    covered: set[tuple[str, str]] = set()
    for original in originals:
        retained = [identifier for identifier in original['packages'] if identifier in current]
        if retained:
            retained_notice = copy.deepcopy(original)
            retained_notice['packages'] = retained
            notices.append(retained_notice)
            covered.update((identifier, original['license']) for identifier in retained)
    errors: list[str] = []
    verified: dict[str, bytes] = {}
    extracted: dict[Path, bytes] = {}
    for identifier, package in sorted(current.items()):
        expression = package.get('license') or ''
        # AND expressions and exceptions require their own reviewed policy.
        licenses = expression.split(' OR ')
        for license_id in licenses:
            if (identifier, license_id) in covered:
                continue
            candidates: list[NoticeCandidate] = []
            for original in originals:
                if original['license'] != license_id:
                    continue
                same_name = any(item.rsplit(' ', 1)[0] == package['name'] for item in original['packages'])
                source = None if 'package_file' in original else source_for(package, original.get('source', ''))
                if not same_name and source is None:
                    continue
                candidates.append(NoticeCandidate(original, source, same_name, source == original.get('source')))
            for match in sorted(candidates, key=NoticeCandidate.priority):
                original = match.notice
                source = match.source
                candidate = copy.deepcopy(original)
                candidate['packages'] = [identifier]
                try:
                    if 'package_file' in original:
                        root = Path(package['manifest_path']).resolve().parent
                        path = (root / original['package_file']).resolve()
                        if not path.is_relative_to(root) or not path.is_file():
                            raise ValueError('Reviewed package notice is missing or escapes its crate')
                    else:
                        if source is None:
                            raise ValueError('Published package repository differs from the reviewed source')
                        verified_text(manifest.parent, original)
                        if source not in verified:
                            verified[source] = fetch_notice(source)
                        data = verified[source]
                        if not data.decode('utf-8').strip():
                            raise ValueError('Upstream notice is empty')
                        digest = hashlib.sha256(data).hexdigest()
                        if digest != original.get('sha256'):
                            filename = f'{package["name"]}-{package["version"]}-{license_id}-{digest[:12]}.txt'
                            relative = Path('notices') / filename
                            path = (manifest.parent / relative).resolve()
                            if not path.is_relative_to((manifest.parent / 'notices').resolve()):
                                raise ValueError('Extracted notice path escapes its directory')
                            candidate.update(file=relative.as_posix(), sha256=digest)
                            candidate.pop('no_final_newline', None)
                            if not data.endswith(b'\n'):
                                candidate['no_final_newline'] = True
                                data += b'\n'
                            extracted[path] = data
                        candidate['source'] = source
                    notices.append(candidate)
                    covered.add((identifier, license_id))
                except (OSError, ValueError) as error:
                    errors.append(f'{identifier} ({license_id}): {error}')
                break
    declarations: list[LicenseDeclaration] = []
    for declaration in configured.get('declarations', []):
        name = declaration['package'].rsplit(' ', 1)[0]
        for identifier, package in sorted(current.items()):
            if package['name'] != name:
                continue
            candidate = copy.deepcopy(declaration)
            try:
                if identifier == declaration['package']:
                    verified_declaration(package, declaration)
                    declarations.append(candidate)
                    continue
                source = source_for(package, declaration['source'])
                if source is None:
                    raise ValueError('Published declaration repository differs from the reviewed source')
                root = Path(package['manifest_path']).resolve().parent
                original_manifest = root / 'Cargo.toml.orig'
                if not original_manifest.resolve().is_relative_to(root):
                    raise ValueError('Published manifest escapes its crate')
                candidate.update(
                    package=identifier, source=source, sha256=hashlib.sha256(original_manifest.read_bytes()).hexdigest()
                )
                verified_declaration(package, candidate)
                declarations.append(candidate)
            except (OSError, ValueError) as error:
                errors.append(f'{identifier}: {error}')
    if errors:
        raise ValueError('License synchronization failed:\n' + '\n'.join(sorted(set(errors))))
    document['notices'] = notices
    document['declarations'] = declarations
    updated = tomlkit.dumps(document)
    for path, data in extracted.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    if updated != manifest.read_text():
        manifest.write_text(updated)
