"""Exercise exact source selection, bounded deterministic bundles, and unresolved license gates."""

import gzip
import io
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from archive_io import open_tar
from hass_image_inventory import ImageFiles
from hass_release_contract import check_shape
from hass_source_parts import part_path
from hass_sources import (
    BundleSources,
    bundle_sources,
    resolve_runtime,
    runtime_coverage,
    source_graph,
    source_members,
    source_paths,
    verify_sources,
    write_bundle,
)
from json_data import Object, file_sha256, json_bytes

SCRATCH = Path(__file__).resolve().parents[2] / '.tmp'
SOURCE = '/nix/store/' + '0' * 32 + '-fixture-source'
RECIPE = '/nix/store/' + '1' * 32 + '-fixture.drv'


class SourceBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        SCRATCH.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='source-tests.', dir=SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / Path(SOURCE).name
        self.source.mkdir()
        (self.source / 'COPYING').write_text('Synthetic license fixture\n')
        self.recipe = self.root / Path(RECIPE).name
        self.recipe.write_text('Synthetic build recipe\n')
        self.archive, self.report = self.root / 'sources.tar.gz', self.root / 'sources.json'
        write_bundle([self.source, self.recipe], self.archive)
        self.document: Object = {
            'schema_version': 2,
            'image_sha256': 'a' * 64,
            'archive_sha256': file_sha256(self.archive),
            'sources': [SOURCE, RECIPE],
            'derivations': {RECIPE: {'inputs': [], 'sources': [SOURCE]}},
            'runtime': [
                {'path': SOURCE, 'derivation': RECIPE, 'resolved': True, 'licenses': [{'id': 'MIT', 'free': True}]}
            ],
            'unresolved': [],
        }
        self.save()

    def save(self) -> None:
        self.report.write_bytes(json_bytes(self.document))

    def test_normalized_bundle_is_deterministic_and_verified_without_extracting(self) -> None:
        second = self.root / 'second.tar.gz'
        write_bundle([self.source, self.recipe], second)
        self.assertEqual(file_sha256(self.archive), file_sha256(second))
        verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)
        self.assertEqual(list(self.root.glob('COPYING')), [])

    def test_source_budgets_are_independent_of_image_budgets(self) -> None:
        with patch('archive_io.MAX_MEMBERS', 1), patch('archive_io.MAX_IMAGE_BYTES', 1024):
            verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)
            with self.assertRaises(ValueError), open_tar(self.archive) as archive:
                list(archive)

    def test_source_writer_and_reader_enforce_member_and_exact_tar_byte_budgets(self) -> None:
        decoded = len(gzip.decompress(self.archive.read_bytes()))
        for name, value in (('MAX_SOURCE_MEMBERS', 2), ('MAX_SOURCE_BYTES', decoded - 1)):
            with self.subTest(name=name), patch('hass_sources.' + name, value):
                destination = self.root / (name + '.tar.gz')
                with (
                    patch('hass_sources.gzip.GzipFile', side_effect=AssertionError('Compression started')),
                    self.assertRaisesRegex(ValueError, 'limit.*sources/'),
                ):
                    write_bundle([self.source, self.recipe], destination)
                self.assertFalse(destination.exists())
                with self.assertRaisesRegex(ValueError, 'size limit'):
                    verify_sources(self.archive, self.report, 'a' * 64)
        with patch('hass_sources.MAX_SOURCE_BYTES', decoded):
            second = self.root / 'at-limit.tar.gz'
            write_bundle([self.source, self.recipe], second)
            self.assertEqual(file_sha256(second), file_sha256(self.archive))
            verify_sources(self.archive, self.report, 'a' * 64)

    def test_metadata_preflight_matches_links_and_pax_headers_without_reading_payloads(self) -> None:
        (self.source / 'COPYING').write_text('Synthetic license fixture\n' * 1024)
        (self.source / 'hardlink').hardlink_to(self.source / 'COPYING')
        (self.source / 'symlink').symlink_to('COPYING')
        nested = self.source / ('long-' + 'x' * 100)
        nested.mkdir()
        (nested / 'file').write_text('Synthetic long-path fixture\n')
        paths = [self.source, self.recipe]
        with patch('tarfile.bltn_open', side_effect=AssertionError('Read a source payload')):
            members = list(source_members(paths))
        self.assertTrue(any(member.islnk() for member in members))
        self.assertTrue(any(member.issym() for member in members))
        original = self.root / 'links.tar.gz'
        write_bundle(paths, original)
        decoded = len(gzip.decompress(original.read_bytes()))
        with (
            patch('hass_sources.MAX_SOURCE_BYTES', decoded),
            patch('hass_sources.MAX_SOURCE_MEMBERS', len(members)),
            patch('builtins.print') as printed,
        ):
            exact = self.root / 'links-exact.tar.gz'
            write_bundle(paths, exact)
        printed.assert_called_once_with(
            f'Dependency sources: {len(members)} members, {decoded} tar bytes; limits {len(members)}, {decoded}',
            flush=True,
        )
        self.assertEqual(file_sha256(exact), file_sha256(original))

    def test_compressed_source_parts_are_bounded_and_verified_as_one_stream(self) -> None:
        directory = self.root / 'split'
        directory.mkdir()
        destination = directory / 'dependency-sources.tar.gz'
        with patch('hass_sources.MAX_BUNDLE_BYTES', 32):
            digest, parts = write_bundle([self.source, self.recipe], destination)
            self.assertGreater(len(parts), 1)
            files = [part_path(destination, number) for number in range(1, len(parts) + 1)]
            self.assertTrue(all(0 < path.stat().st_size <= 32 for path in files))
            self.assertEqual(b''.join(path.read_bytes() for path in files), self.archive.read_bytes())
            self.document.update(archive_sha256=digest, parts=parts)
            self.save()
            (directory / 'dependency-sources.json').write_bytes(json_bytes(self.document))
            check_shape(directory, 'hass-sources.dirspec.toml')
            verify_sources(destination, self.report, 'a' * 64, part_count=len(parts), require_resolved=True)
            with self.assertRaisesRegex(ValueError, 'release inventory'):
                verify_sources(destination, self.report, 'a' * 64, part_count=len(parts) + 1)
            original = files[1].read_bytes()
            files[1].write_bytes(bytes([original[0] ^ 1]) + original[1:])
            with self.assertRaisesRegex(ValueError, 'part differs'):
                verify_sources(destination, self.report, 'a' * 64)
            files[1].write_bytes(original)
            files[-1].unlink()
            with self.assertRaises(FileNotFoundError):
                verify_sources(destination, self.report, 'a' * 64)

    def test_source_part_count_limit_leaves_no_oversized_files(self) -> None:
        destination = self.root / 'too-many-parts.tar.gz'
        with (
            patch('hass_sources.MAX_BUNDLE_BYTES', 32),
            patch('hass_source_parts.MAX_PARTS', 1),
            self.assertRaisesRegex(ValueError, 'part count limit'),
        ):
            write_bundle([self.source, self.recipe], destination)
        self.assertLessEqual(destination.stat().st_size, 32)

    def test_source_selection_handles_current_structured_and_legacy_derivations(self) -> None:
        for recipe in (
            {'inputs': {'srcs': [Path(SOURCE).name]}, 'env': {}, 'structuredAttrs': {'src': SOURCE}},
            {'inputSrcs': [SOURCE], 'env': {'src': SOURCE}},
        ):
            with self.subTest(recipe=recipe):
                self.assertEqual(source_paths({RECIPE: recipe}), [Path(SOURCE), Path(RECIPE)])
        with self.assertRaisesRegex(ValueError, 'canonical'):
            source_paths({RECIPE: {'inputSrcs': ['/etc/passwd'], 'env': {}}})

    def test_unknown_or_nonfree_runtime_providers_remain_unresolved(self) -> None:
        runtime: Object = {SOURCE: {'deriver': RECIPE}}
        for licenses in ([], [{'id': 'NOASSERTION', 'free': True}], [{'id': 'LicenseRef-restricted', 'free': False}]):
            with self.subTest(licenses=licenses):
                rows = runtime_coverage(runtime, [{'derivation': RECIPE, 'licenses': licenses}])
                self.assertIs(rows[0]['resolved'], False)
                self.document.update(runtime=rows, unresolved=[SOURCE])
                self.save()
                verify_sources(self.archive, self.report, 'a' * 64)
                with self.assertRaisesRegex(ValueError, 'unresolved'):
                    verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)

    def test_missing_store_derivers_use_exact_package_outputs_including_nondefault_outputs(self) -> None:
        metadata: list[Object] = [
            {'derivation': RECIPE, 'outputs': [SOURCE + '-out', SOURCE], 'licenses': [{'id': 'MIT', 'free': True}]}
        ]
        for row in ({}, {'deriver': None}, {'deriver': ''}):
            with self.subTest(row=row):
                runtime = resolve_runtime({SOURCE: row}, metadata)
                self.assertEqual(runtime[SOURCE], {'deriver': RECIPE})
                self.assertIs(runtime_coverage(runtime, metadata)[0]['resolved'], True)
        recorded: Object = {SOURCE: {'deriver': RECIPE}}
        self.assertEqual(resolve_runtime(recorded, []), recorded)

    def test_cached_derivers_are_replaced_by_the_current_exact_output_provider(self) -> None:
        cached = '/nix/store/' + '2' * 32 + '-cached.drv'
        metadata: list[Object] = [
            {'derivation': RECIPE, 'outputs': [SOURCE + '-out', SOURCE], 'licenses': [{'id': 'MIT', 'free': True}]}
        ]
        runtime = resolve_runtime({SOURCE: {'deriver': cached, 'narSize': 128}}, metadata)
        self.assertEqual(runtime[SOURCE], {'deriver': RECIPE, 'narSize': 128})
        self.assertIs(runtime_coverage(runtime, metadata)[0]['resolved'], True)
        metadata.append({'derivation': cached, 'outputs': [SOURCE]})
        with self.assertRaisesRegex(ValueError, 'unique.*' + SOURCE):
            resolve_runtime({SOURCE: {'deriver': RECIPE}}, metadata)

    def test_missing_or_ambiguous_output_matches_and_invalid_derivers_fail_closed(self) -> None:
        cases: list[list[Object]] = [
            [],
            [{'derivation': RECIPE, 'outputs': [SOURCE + '-different']}],
            [
                {'derivation': RECIPE, 'outputs': [SOURCE]},
                {'derivation': RECIPE.replace('fixture.drv', 'other.drv'), 'outputs': [SOURCE]},
            ],
        ]
        for metadata in cases:
            with self.subTest(metadata=metadata), self.assertRaisesRegex(ValueError, 'unique.*' + SOURCE):
                resolve_runtime({SOURCE: {}}, metadata)
        with self.assertRaisesRegex(ValueError, 'invalid.*' + SOURCE):
            resolve_runtime({SOURCE: {'deriver': '/outside/recipe.drv'}}, [])

    def test_runtime_wrapper_requires_its_upstream_sources_even_with_free_licenses(self) -> None:
        wrapper = '/nix/store/' + '2' * 32 + '-runtime-wrapper.drv'
        recipes: Object = {
            wrapper: {'env': {}, 'inputDrvs': {RECIPE: ['out']}},
            RECIPE: {'env': {'src': SOURCE}, 'inputSrcs': [SOURCE]},
        }
        self.assertEqual(set(source_paths(recipes)), {Path(SOURCE), Path(RECIPE), Path(wrapper)})
        wrapper_file = self.root / Path(wrapper).name
        wrapper_file.write_text('Synthetic wrapper recipe\n')
        self.archive.unlink()
        write_bundle([self.recipe, wrapper_file], self.archive)
        self.document.update(
            archive_sha256=file_sha256(self.archive),
            sources=[RECIPE, wrapper],
            derivations=source_graph(recipes),
            runtime=[
                {'path': SOURCE, 'derivation': wrapper, 'resolved': True, 'licenses': [{'id': 'MIT', 'free': True}]}
            ],
        )
        self.save()
        with self.assertRaisesRegex(ValueError, 'upstream sources'):
            verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)
        self.archive.unlink()
        write_bundle([self.source, self.recipe, wrapper_file], self.archive)
        self.document.update(archive_sha256=file_sha256(self.archive), sources=[SOURCE, RECIPE, wrapper])
        self.save()
        verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)
        self.document['derivations'] = {wrapper: {'inputs': [RECIPE], 'sources': []}}
        self.save()
        with self.assertRaisesRegex(ValueError, 'missing input recipes'):
            verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)

    def test_recipe_only_coverage_is_rejected_and_fixed_output_sources_are_selected(self) -> None:
        self.document['derivations'] = {RECIPE: {'inputs': [], 'sources': []}}
        self.save()
        with self.assertRaisesRegex(ValueError, 'no upstream sources'):
            verify_sources(self.archive, self.report, 'a' * 64, require_resolved=True)
        for recipe in (
            {'env': {}, 'outputs': {'out': {'path': SOURCE, 'hash': 'sha256-fixture'}}},
            {'env': {'out': SOURCE}, 'outputs': {'out': {'hash': 'sha256-fixture', 'method': 'flat'}}},
        ):
            with self.subTest(recipe=recipe):
                self.assertIn(Path(SOURCE), source_paths({RECIPE: recipe}))

    def test_archive_hash_and_image_identity_are_required(self) -> None:
        with self.assertRaisesRegex(ValueError, 'image'):
            verify_sources(self.archive, self.report, 'b' * 64)
        self.archive.write_bytes(self.archive.read_bytes() + b'altered')
        with self.assertRaisesRegex(ValueError, 'archive'):
            verify_sources(self.archive, self.report, 'a' * 64)

    def test_traversal_duplicate_and_missing_source_roots_are_rejected(self) -> None:
        for names in (['../outside'], ['sources/' + Path(SOURCE).name] * 2, ['sources/unexpected']):
            with self.subTest(names=names):
                with tarfile.open(self.archive, 'w:gz') as archive:
                    for name in names:
                        member = tarfile.TarInfo(name)
                        member.size = 1
                        archive.addfile(member, io.BytesIO(b'x'))
                self.document['archive_sha256'] = file_sha256(self.archive)
                self.save()
                with self.assertRaises(ValueError):
                    verify_sources(self.archive, self.report, 'a' * 64)

    def test_realisation_never_requests_build_outputs_for_recipe_files(self) -> None:
        missing = self.root / Path(SOURCE).name.replace('fixture-source', 'missing-source')
        metadata = self.root / 'metadata.json'
        metadata.write_bytes(json_bytes([{'derivation': RECIPE, 'licenses': [{'id': 'MIT', 'free': True}]}]))
        runtime: Object = {SOURCE: {'deriver': RECIPE}}
        commands: list[list[str]] = []

        def realised(arguments: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
            commands.append(arguments)
            missing.write_text('Synthetic missing source\n')
            return subprocess.CompletedProcess(arguments, 0)

        with (
            patch('hass_sources.inspect_archive'),
            patch(
                'hass_sources.inventory_image',
                return_value=ImageFiles(hashes={}, store_paths={SOURCE}, wasm={}, native={}),
            ),
            patch('hass_sources.nix_json', return_value=runtime),
            patch('hass_sources.derivations', return_value={RECIPE: {'env': {}, 'outputs': {}}}),
            patch('hass_sources.required_sources', return_value=[self.recipe, missing]),
            patch('hass_sources.subprocess.run', side_effect=realised),
            patch('hass_sources.check_shape'),
        ):
            bundle_sources(
                BundleSources(
                    archive=self.source / 'COPYING', metadata=metadata, output=self.root / 'bundle', realise=True
                )
            )
        self.assertEqual(commands, [['nix-store', '--realise', str(missing)]])

    def test_store_links_are_relocated_and_external_links_are_rejected(self) -> None:
        (self.source / 'recipe-link').symlink_to(RECIPE)
        relocated = self.root / 'relocated.tar.gz'
        write_bundle([self.source, self.recipe], relocated)
        with tarfile.open(relocated) as archive:
            link = archive.getmember('sources/' + self.source.name + '/recipe-link')
            self.assertEqual(link.linkname, '../' + self.recipe.name)
        (self.source / 'outside-link').symlink_to('/etc/passwd')
        with self.assertRaisesRegex(ValueError, 'escapes'):
            write_bundle([self.source, self.recipe], self.root / 'unsafe.tar.gz')
