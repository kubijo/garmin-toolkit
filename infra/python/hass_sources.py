"""Bundle exact Nix build sources and report unresolved runtime redistribution coverage."""

import gzip
import hashlib
import io
import json
import posixpath
import re
import subprocess
import tarfile
import tempfile
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from archive_io import chunks, open_tar
from haos_platform import inspect_archive
from hass_image_inventory import inventory_image
from hass_release_contract import check_shape
from hass_source_parts import MAX_PARTS, PartsReader, PartsWriter, part_path
from json_data import Object, file_sha256, json_bytes, object_rows, object_value, read_json, strings, text

STORE_PATH = r'/nix/store/[0-9a-df-np-sv-z]{32}-[A-Za-z0-9+._?=-]+'
MAX_BUNDLE_BYTES = 1536 * 1024**2
MAX_SOURCE_BYTES = 32 * 1024**3
MAX_SOURCE_MEMBERS = 2_000_000


@dataclass(frozen=True, kw_only=True)
class BundleSources:
    """Export build sources; unknown runtime licenses remain explicit publication blockers."""

    archive: Path
    metadata: Path
    output: Path
    realise: bool = False


def nix_json(*arguments: str) -> Object:
    result = subprocess.run(['nix', *arguments], check=True, stdout=subprocess.PIPE, text=True, timeout=180)
    return object_value(json.loads(result.stdout))


def derivations(paths: list[str]) -> Object:
    document = nix_json('derivation', 'show', '--recursive', *paths)
    if 'derivations' in document:
        if document.get('version') != 4:
            raise ValueError('Unsupported Nix derivation JSON version')
        return {'/nix/store/' + name: value for name, value in object_value(document['derivations']).items()}
    return document


def source_graph(recipes: Object) -> Object:
    graph: Object = {}
    for name, value in recipes.items():
        recipe = object_value(value)
        if 'inputs' in recipe:
            inputs = object_value(recipe['inputs'])
            sources = {'/nix/store/' + name for name in strings(inputs.get('srcs', []))}
            dependencies = ['/nix/store/' + name for name in object_value(inputs.get('drvs', {}))]
        else:
            sources = set(strings(recipe.get('inputSrcs', [])))
            dependencies = list(object_value(recipe.get('inputDrvs', {})))
        environment = {**object_value(recipe.get('env')), **object_value(recipe.get('structuredAttrs', {}))}
        for key in ('src', 'srcs', 'patches', 'cargoVendorDir'):
            value = environment.get(key)
            if value is not None:
                sources.update(re.findall(STORE_PATH, json.dumps(value)))
        for output_name, output_value in object_value(recipe.get('outputs', {})).items():
            output = object_value(output_value)
            if environment.get('outputHash') or 'hash' in output:
                sources.add(text(output.get('path', environment.get(output_name))))
        graph[name] = {'inputs': sorted(dependencies), 'sources': sorted(sources)}
    return graph


def required_sources(graph: Object) -> list[Path]:
    sources = set(graph)
    for name, value in graph.items():
        row = object_value(value)
        if not name.endswith('.drv') or not set(strings(row.get('inputs'))).issubset(graph):
            raise ValueError('Dependency source graph has missing input recipes')
        sources.update(strings(row.get('sources')))
    if not sources or any(re.fullmatch(STORE_PATH, path) is None for path in sources):
        raise ValueError('Expected complete, canonical Nix source and derivation paths')
    return [Path(path) for path in sorted(sources)]


def source_paths(recipes: Object) -> list[Path]:
    return required_sources(source_graph(recipes))


def resolve_runtime(runtime: Object, metadata: list[Object]) -> Object:
    """Prefer exact evaluated outputs over potentially unavailable cache derivers."""
    providers: dict[str, set[str]] = {}
    for row in metadata:
        for path in strings(row.get('outputs', [])):
            providers.setdefault(path, set()).add(text(row.get('derivation')))
    result: Object = {}
    for path, value in runtime.items():
        row = object_value(value)
        producing = row.get('deriver')
        candidates = providers.get(path, set())
        if candidates or producing in (None, ''):
            if len(candidates) != 1:
                raise ValueError('Runtime output has no unique producing derivation: ' + path)
            producing = next(iter(candidates))
        if not isinstance(producing, str) or re.fullmatch(STORE_PATH + r'\.drv', producing) is None:
            raise ValueError('Runtime output has an invalid producing derivation: ' + path)
        result[path] = {**row, 'deriver': producing}
    return result


def runtime_coverage(runtime: Object, metadata: list[Object]) -> list[Object]:
    indexed = {text(row.get('derivation')): row for row in metadata}
    result: list[Object] = []
    for path, value in sorted(runtime.items()):
        producing = text(object_value(value).get('deriver', ''))
        provider = indexed.get(producing, {})
        licenses = object_rows(provider.get('licenses', []))
        result.append(
            {
                'path': path,
                'derivation': producing,
                'licenses': licenses,
                'resolved': licenses_resolved(licenses),
            }
        )
    return result


def licenses_resolved(licenses: list[Object]) -> bool:
    return bool(licenses) and all(
        row.get('free') is True and isinstance(row.get('id'), str) and row.get('id') not in ('', 'NOASSERTION')
        for row in licenses
    )


def source_members(paths: list[Path]) -> Iterator[tarfile.TarInfo]:
    """Match tarfile's traversal and hardlink accounting without reading file payloads."""
    with tarfile.open(fileobj=io.BytesIO(), mode='w', dereference=False) as archive:

        def walk(path: Path, name: str) -> Iterator[tarfile.TarInfo]:
            # CPython returns None for unsupported types, despite the narrower stub.
            member = cast(tarfile.TarInfo | None, archive.gettarinfo(path, name))
            if member is None:
                raise ValueError('Unsupported dependency source file type: ' + str(path))
            yield member
            if member.isdir():
                for child in sorted(path.iterdir()):
                    yield from walk(child, name + '/' + child.name)

        for path in paths:
            yield from walk(path, 'sources/' + path.name)


def write_bundle(paths: list[Path], destination: Path) -> tuple[str, list[Object]]:
    count, total, finished = 0, 0, 0
    roots = {'sources/' + path.name for path in paths}

    def normalized(member: tarfile.TarInfo) -> tarfile.TarInfo:
        nonlocal count, total, finished
        count += 1
        if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
            raise ValueError('Unsupported dependency source file type')
        if member.issym() and member.linkname.startswith('/nix/store/'):
            target = 'sources/' + member.linkname.removeprefix('/nix/store/')
            member.linkname = posixpath.relpath(target, posixpath.dirname(member.name))
        if member.issym() or member.islnk():
            check_link(member, roots)
        member.uid = member.gid = member.mtime = 0
        member.uname = member.gname = ''
        member.pax_headers = {}
        block, record = tarfile.BLOCKSIZE, tarfile.RECORDSIZE
        total += len(member.tobuf(format=tarfile.PAX_FORMAT)) + (member.size + block - 1) // block * block
        finished = (total + 2 * block + record - 1) // record * record
        if count > MAX_SOURCE_MEMBERS or finished > MAX_SOURCE_BYTES:
            raise ValueError(
                f'Dependency source limit exceeded at {member.name}: '
                f'{count}/{MAX_SOURCE_MEMBERS} members, {finished}/{MAX_SOURCE_BYTES} tar bytes'
            )
        return member

    for member in source_members(paths):
        normalized(member)
    print(
        f'Dependency sources: {count} members, {finished} tar bytes; limits {MAX_SOURCE_MEMBERS}, {MAX_SOURCE_BYTES}',
        flush=True,
    )
    count, total, finished = 0, 0, 0
    with (
        PartsWriter(destination, MAX_BUNDLE_BYTES) as raw,
        gzip.GzipFile(fileobj=raw, mode='wb', filename='', mtime=0) as compressed,
        tarfile.open(fileobj=compressed, mode='w|', dereference=False) as archive,
    ):
        for path in paths:
            archive.add(path, arcname='sources/' + path.name, filter=normalized)
    return raw.digest.hexdigest(), [{'size': path.stat().st_size, 'sha256': file_sha256(path)} for path in raw.paths]


def check_link(member: tarfile.TarInfo, roots: set[str]) -> None:
    target = posixpath.normpath(
        member.linkname if member.islnk() else posixpath.join(posixpath.dirname(member.name), member.linkname)
    )
    if '/'.join(target.split('/')[:2]) not in roots:
        raise ValueError('Dependency source link escapes the bundled source roots')


def bundle_sources(options: BundleSources) -> None:
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('Dependency source output already exists')
    inspected = inspect_archive(options.archive)
    files = inventory_image(options.archive, inspected)
    runtime = nix_json('path-info', '--json', '--json-format', '1', *sorted(files.store_paths))
    if set(runtime) != files.store_paths:
        raise ValueError('Nix runtime inventory differs from the image')
    metadata = object_rows(read_json(options.metadata))
    runtime = resolve_runtime(runtime, metadata)
    roots = sorted({text(object_value(row).get('deriver', '')) for row in runtime.values()})
    if not roots or any(re.fullmatch(STORE_PATH + r'\.drv', path) is None for path in roots):
        raise ValueError('Runtime outputs have missing producing derivations')
    recipes = derivations(roots)
    graph = source_graph(recipes)
    paths = required_sources(graph)
    missing = [path for path in paths if not path.exists()]
    if any(path.suffix == '.drv' for path in missing):
        raise ValueError('Source derivation recipes disappeared during the export')
    if options.realise and missing:
        subprocess.run(
            ['nix-store', '--realise', *map(str, missing)],
            check=True,
            timeout=1800,
            stdout=subprocess.DEVNULL,
        )
    if any(not path.exists() for path in paths):
        raise ValueError('Dependency sources are missing; realise them with bounded Nix workers first')
    vendors: set[str] = set()
    selected = {str(path) for path in paths}
    for recipe in recipes.values():
        settings = {
            **object_value(object_value(recipe).get('env')),
            **object_value(object_value(recipe).get('structuredAttrs', {})),
        }
        value = settings.get('cargoVendorDir')
        if value is not None:
            vendors.update(path for path in re.findall(STORE_PATH, json.dumps(value)) if path in selected)
    if vendors:
        closure = nix_json('path-info', '--json', '--json-format', '1', '--recursive', *sorted(vendors))
        paths = sorted(set(paths) | {Path(path) for path in closure})
    coverage = runtime_coverage(runtime, metadata)
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-sources.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'sources'
        staged.mkdir()
        archive = staged / 'dependency-sources.tar.gz'
        archive_hash, parts = write_bundle(paths, archive)
        document: Object = {
            'schema_version': 2,
            'image_sha256': file_sha256(options.archive),
            'archive_sha256': archive_hash,
            'parts': parts,
            'sources': [str(path) for path in paths],
            'derivations': graph,
            'runtime': coverage,
            'unresolved': [row['path'] for row in coverage if not row['resolved']],
        }
        (staged / 'dependency-sources.json').write_bytes(json_bytes(document))
        check_shape(staged, 'hass-sources.dirspec.toml')
        staged.rename(options.output)


def source_part_count(document: Object) -> int:
    parts = document.get('parts')
    count = 1 if parts is None else len(object_rows(parts))
    if not 1 <= count <= MAX_PARTS:
        raise ValueError('Invalid dependency source part count')
    return count


def verify_sources(
    archive: Path, report: Path, image_hash: str, *, require_resolved: bool = False, part_count: int | None = None
) -> None:
    document = object_value(read_json(report))
    if document.get('schema_version') != 2 or document.get('image_sha256') != image_hash:
        raise ValueError('Dependency source inventory differs from its image')
    count = source_part_count(document)
    if part_count is not None and count != part_count:
        raise ValueError('Dependency source parts differ from the release inventory')
    files = [part_path(archive, number) for number in range(1, count + 1)]
    records = object_rows(
        document.get('parts', [{'sha256': document.get('archive_sha256'), 'size': archive.stat().st_size}])
    )
    digest = hashlib.sha256()
    for path, record in zip(files, records, strict=True):
        size = path.lstat().st_size
        if path.is_symlink() or not path.is_file() or not 0 < size <= MAX_BUNDLE_BYTES or size != record.get('size'):
            raise ValueError('Dependency source part exceeds its size limit or differs from its inventory')
        part_hash = hashlib.sha256()
        with path.open('rb') as stream:
            for data in chunks(stream):
                part_hash.update(data)
                digest.update(data)
        if part_hash.hexdigest() != record.get('sha256'):
            raise ValueError('Dependency source archive part differs from its inventory')
    if digest.hexdigest() != document.get('archive_sha256'):
        raise ValueError('Dependency source archive differs from its inventory')
    paths = strings(document.get('sources'))
    if len(set(paths)) != len(paths) or any(re.fullmatch(STORE_PATH, path) is None for path in paths):
        raise ValueError('Dependency inventory has duplicate or noncanonical source paths')
    graph = object_value(document.get('derivations'))
    if not {str(path) for path in required_sources(graph)}.issubset(paths):
        raise ValueError('Dependency source archive omits required upstream sources')
    expected = {'sources/' + Path(path).name for path in paths}
    seen: set[str] = set()
    with (
        PartsReader(files) as parts,
        gzip.GzipFile(fileobj=parts) as decoded,
        open_tar(decoded, max_bytes=MAX_SOURCE_BYTES, max_members=MAX_SOURCE_MEMBERS) as source,
    ):
        for member in source:
            path = Path(member.name)
            if path.is_absolute() or '..' in path.parts or len(path.parts) < 2:
                raise ValueError('Unsafe dependency source archive path')
            root = '/'.join(path.parts[:2])
            if root not in expected or member.name in seen:
                raise ValueError('Unexpected or duplicate dependency source archive entry')
            seen.add(member.name)
            if member.isfile():
                for _ in chunks(source.extractfile(member)):
                    pass
            elif not (member.isdir() or member.issym() or member.islnk()):
                raise ValueError('Unsupported dependency source archive member')
            if member.issym() or member.islnk():
                check_link(member, expected)
    if not expected or not expected.issubset(seen):
        raise ValueError('Dependency source archive is incomplete')
    runtime = object_rows(document.get('runtime'))
    runtime_paths: set[str] = set()
    for row in runtime:
        path, recipe = text(row.get('path')), text(row.get('derivation'))
        if not re.fullmatch(STORE_PATH, path) or path in runtime_paths or recipe not in paths or recipe not in graph:
            raise ValueError('Runtime source inventory has missing or duplicate producing recipes')
        runtime_paths.add(path)
        pending, visited, upstream = [recipe], set[str](), set[str]()
        while pending:
            current = pending.pop()
            if current in visited:
                continue
            visited.add(current)
            node = object_value(graph[current])
            upstream.update(path for path in strings(node['sources']) if not path.endswith('.drv'))
            pending.extend(strings(node['inputs']))
        if not upstream:
            raise ValueError('Runtime source inventory has no upstream sources')
        licenses = object_rows(row.get('licenses', []))
        resolved = licenses_resolved(licenses)
        if row.get('resolved') != resolved:
            raise ValueError('Runtime license resolution differs from its declared licenses')
    unresolved = [row.get('path') for row in runtime if row.get('resolved') is not True]
    if not runtime or document.get('unresolved') != unresolved or (require_resolved and unresolved):
        raise ValueError('Runtime redistribution coverage remains unresolved')
