"""Measure AppImage contents and export their exact Nix sources without executing foreign binaries."""

import hashlib
import json
import os
import re
import selectors
import shutil
import subprocess
import tempfile
from pathlib import Path
from time import monotonic

from hass_source_parts import part_path
from hass_sources import STORE_PATH, BundleSources, bundle_store_sources, source_part_count, verify_sources
from json_data import Object, file_sha256, json_bytes, object_rows, object_value, read_json, strings, text

MAX_RUNTIME_BYTES = 16 * 1024**2
MAX_STORE_PATHS = 10_000
MAX_LISTING_BYTES = 4 * 1024**2
INSPECTION_TIMEOUT = 180


def names(architecture: str) -> tuple[str, str, str]:
    stem = f'garmin-toolkit-{architecture}'
    return f'{stem}-runtime.json', f'{stem}-sources.json', f'{stem}-sources.tar.gz'


def store_root(path: str) -> str:
    match = re.match(STORE_PATH + r'(?=/|$)', path)
    if match is None:
        raise ValueError('Invalid AppImage component store path')
    return match[0]


def prefix_hash(image: Path, size: int) -> str:
    if not 0 < size <= MAX_RUNTIME_BYTES or size >= image.stat().st_size:
        raise ValueError('Invalid AppImage runtime size')
    with image.open('rb') as stream:
        return hashlib.sha256(stream.read(size)).hexdigest()


def squashfs(image: Path, offset: int, *arguments: str, limit: int, member: str | None = None) -> bytes:
    command = ['unsquashfs', '-no-progress', '-offset', str(offset), *arguments, str(image)]
    if member is not None:
        command.append(member)
    deadline = monotonic() + INSPECTION_TIMEOUT
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
        assert process.stdout is not None and process.stderr is not None
        output, errors = bytearray(), bytearray()
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                selector.register(process.stderr, selectors.EVENT_READ)
                while selector.get_map():
                    remaining = deadline - monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(command, INSPECTION_TIMEOUT)
                    for key, _ in selector.select(remaining):
                        target, bound = (output, limit) if key.fileobj is process.stdout else (errors, 64 * 1024)
                        data = os.read(key.fd, min(64 * 1024, bound - len(target) + 1))
                        if not data:
                            selector.unregister(key.fileobj)
                        else:
                            target.extend(data)
                            if len(target) > bound:
                                raise ValueError('SquashFS output exceeds its size limit')
            status = process.wait(timeout=max(0, deadline - monotonic()))
            if status:
                raise subprocess.CalledProcessError(status, command, bytes(output), bytes(errors))
            return bytes(output)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


def measured_store_paths(image: Path, offset: int) -> set[str]:
    # Listing only three levels bounds work to the store roots, not every file in every package.
    listing = squashfs(image, offset, '-ls', '-max-depth', '3', limit=MAX_LISTING_BYTES).decode('utf-8')
    paths = {
        '/nix/store/' + match[1]
        for line in listing.splitlines()
        if (match := re.fullmatch(r'squashfs-root/nix/store/([^/ ]+)(?: -> .*)?', line))
    }
    if not paths or len(paths) > MAX_STORE_PATHS or any(re.fullmatch(STORE_PATH, path) is None for path in paths):
        raise ValueError('Invalid AppImage store inventory')
    return paths


def app_run_hash(image: Path, offset: int) -> str:
    data = squashfs(image, offset, '-cat', member='AppRun', limit=MAX_RUNTIME_BYTES)
    if not data:
        raise ValueError('Invalid AppRun size')
    return hashlib.sha256(data).hexdigest()


def asset_names(directory: Path, architecture: str) -> set[str]:
    runtime, report, archive = names(architecture)
    count = source_part_count(object_value(read_json(directory / report)))
    return {runtime, report} | {part_path(Path(archive), number).name for number in range(1, count + 1)}


def verify(directory: Path, architecture: str, tag: str, commit: str | None = None) -> None:
    runtime_name, report_name, archive_name = names(architecture)
    inventory_path = directory / runtime_name
    inventory = object_value(read_json(inventory_path))
    if set(inventory) != {'schema_version', 'system', 'tag', 'commit', 'images'} or inventory['schema_version'] != 1:
        raise ValueError('Invalid desktop runtime inventory')
    if inventory['system'] != f'{architecture}-linux' or inventory['tag'] != tag:
        raise ValueError('Desktop runtime inventory differs from its release')
    if not re.fullmatch(r'[0-9a-f]{40}', text(inventory['commit'])) or (commit and inventory['commit'] != commit):
        raise ValueError('Desktop runtime inventory differs from the pinned source')
    images = object_value(inventory['images'])
    if set(images) != {f'{mode}-{architecture}.AppImage' for mode in ('garmin-toolkit', 'garmin-toolkit-demo')}:
        raise ValueError('Desktop runtime inventory must cover both application modes')
    runtime_paths: set[str] = set()
    for name, value in images.items():
        row = object_value(value)
        if set(row) != {
            'sha256',
            'offset',
            'runtime',
            'runtime_sha256',
            'app_run',
            'app_run_sha256',
            'store_paths',
            'static_libraries',
        }:
            raise ValueError('Invalid AppImage component inventory')
        image = directory / name
        offset = row['offset']
        if type(offset) is not int or file_sha256(image) != row['sha256']:
            raise ValueError('Desktop runtime inventory differs from its AppImage')
        if prefix_hash(image, offset) != row['runtime_sha256'] or app_run_hash(image, offset) != row['app_run_sha256']:
            raise ValueError('AppImage runtime or AppRun differs from its inventory')
        paths = strings(row['store_paths'])
        if paths != sorted(set(paths)) or set(paths) != measured_store_paths(image, offset):
            raise ValueError('AppImage store paths differ from its measured inventory')
        runtime_paths.update(paths)
        runtime_paths.update((store_root(text(row['runtime'])), store_root(text(row['app_run']))))
        libraries = strings(row['static_libraries'])
        if not libraries or libraries != sorted(set(libraries)) or any(store_root(path) != path for path in libraries):
            raise ValueError('Invalid static library inventory')
        runtime_paths.update(libraries)
    report = object_value(read_json(directory / report_name))
    if {text(row.get('path')) for row in object_rows(report.get('runtime'))} != runtime_paths:
        raise ValueError('Source inventory differs from the complete AppImage runtime')
    verify_sources(
        directory / archive_name, directory / report_name, file_sha256(inventory_path), require_resolved=True
    )


def export(directory: Path, system: str, tag: str, commit: str) -> None:
    architecture = system.removesuffix('-linux')
    runtime_name, report_name, archive_name = names(architecture)
    components = object_value(json_eval(system, 'desktop-source-components'))
    images: Object = {}
    paths: set[str] = set()
    for mode, key in [('garmin-toolkit', 'production'), ('garmin-toolkit-demo', 'demo')]:
        component = object_value(components[key])
        runtime, app_run = Path(text(component['runtime'])), Path(text(component['appRun']))
        image = directory / f'{mode}-{architecture}.AppImage'
        offset = runtime.stat().st_size
        if prefix_hash(image, offset) != file_sha256(runtime) or app_run_hash(image, offset) != file_sha256(app_run):
            raise ValueError('AppImage differs from its evaluated runtime components')
        measured = measured_store_paths(image, offset)
        paths.update(measured)
        paths.update((store_root(str(runtime)), store_root(str(app_run))))
        libraries = sorted(set(strings(component['staticLibraries'])))
        paths.update(libraries)
        images[image.name] = {
            'sha256': file_sha256(image),
            'offset': offset,
            'runtime': str(runtime),
            'runtime_sha256': file_sha256(runtime),
            'app_run': str(app_run),
            'app_run_sha256': file_sha256(app_run),
            'store_paths': sorted(measured),
            'static_libraries': libraries,
        }
    inventory = directory / runtime_name
    inventory.write_bytes(
        json_bytes({'schema_version': 1, 'system': system, 'tag': tag, 'commit': commit, 'images': images})
    )
    with tempfile.TemporaryDirectory(prefix='.desktop-sources.', dir=directory.parent) as temporary:
        scratch = Path(temporary)
        metadata = scratch / 'metadata.json'
        metadata.write_bytes(json_bytes(json_eval(system, 'desktop-source-metadata')))
        bundle = scratch / 'bundle'
        bundle_store_sources(
            BundleSources(archive=inventory, metadata=metadata, output=bundle, realise=True, require_resolved=True),
            paths,
        )
        report = object_value(read_json(bundle / 'dependency-sources.json'))
        shutil.copyfile(bundle / 'dependency-sources.json', directory / report_name)
        for number in range(1, source_part_count(report) + 1):
            shutil.copyfile(
                part_path(bundle / 'dependency-sources.tar.gz', number), part_path(directory / archive_name, number)
            )
    verify(directory, architecture, tag, commit)


def json_eval(system: str, attribute: str) -> object:
    return json.loads(
        subprocess.run(
            ['nix', 'eval', '--json', f'.#legacyPackages.{system}.{attribute}', '--no-update-lock-file'],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            timeout=180,
        ).stdout
    )
