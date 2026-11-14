"""Small synthetic source inventories for desktop release contract tests."""

import tempfile
from pathlib import Path

import desktop_sources
from hass_sources import write_bundle
from json_data import Object, file_sha256, json_bytes

STORE_PATH = '/nix/store/' + '0' * 32 + '-fixture'
RUNTIME = '/nix/store/' + '2' * 32 + '-runtime'
APP_RUN = '/nix/store/' + '3' * 32 + '-apprun'
STATIC_LIBRARY = '/nix/store/' + '5' * 32 + '-static-library'
RECIPE = '/nix/store/' + '1' * 32 + '-fixture.drv'
SOURCE = '/nix/store/' + '4' * 32 + '-source'


def source_assets(
    directory: Path, tag: str, commit: str, *, offset: int | None = None, app_run_sha256: str = 'a' * 64
) -> None:
    for architecture in ('x86_64', 'aarch64'):
        images: Object = {}
        for mode in ('garmin-toolkit', 'garmin-toolkit-demo'):
            image = directory / f'{mode}-{architecture}.AppImage'
            size = offset if offset is not None else image.stat().st_size - 1
            images[image.name] = {
                'sha256': file_sha256(image),
                'offset': size,
                'runtime': RUNTIME,
                'runtime_sha256': desktop_sources.prefix_hash(image, size),
                'app_run': APP_RUN + '/AppRun',
                'app_run_sha256': app_run_sha256,
                'store_paths': [STORE_PATH],
                'static_libraries': [STATIC_LIBRARY],
            }
        runtime_name, report_name, archive_name = desktop_sources.names(architecture)
        for previous in directory.glob(archive_name + '*'):
            previous.unlink()
        inventory = directory / runtime_name
        inventory.write_bytes(
            json_bytes(
                {
                    'schema_version': 1,
                    'system': f'{architecture}-linux',
                    'tag': tag,
                    'commit': commit,
                    'images': images,
                }
            )
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, recipe = root / Path(SOURCE).name, root / Path(RECIPE).name
            source.mkdir()
            (source / 'COPYING').write_text('Synthetic test notice, not a third-party grant.\n')
            recipe.write_text('Synthetic test recipe.\n')
            digest, parts = write_bundle([source, recipe], directory / archive_name)
        report: Object = {
            'schema_version': 2,
            'image_sha256': file_sha256(inventory),
            'archive_sha256': digest,
            'parts': parts,
            'sources': [RECIPE, SOURCE],
            'derivations': {RECIPE: {'inputs': [], 'sources': [SOURCE]}},
            'runtime': [
                {'path': path, 'derivation': RECIPE, 'resolved': True, 'licenses': [{'id': 'MIT', 'free': True}]}
                for path in [STORE_PATH, RUNTIME, APP_RUN, STATIC_LIBRARY]
            ],
            'unresolved': [],
        }
        (directory / report_name).write_bytes(json_bytes(report))
