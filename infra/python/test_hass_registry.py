"""Verify real native exports and registry transport, plus adversarial OCI and evidence cases."""

import copy
import gzip
import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from fixtures.auditable import elf, wasm
from fixtures.images import write_image
from fixtures.registry import registry
from hass_image_sbom import generate
from hass_registry import (
    INDEX,
    MANIFEST,
    PrepareRegistry,
    VerifyRegistry,
    annotations,
    bind_acceptance,
    metadata,
    native_inputs,
    prepare_registry,
    root_descriptor,
    verify_layout,
    verify_registry,
    write_blob,
)
from hass_release_build import NativeBuild, record_build
from json_data import Object, json_bytes, object_rows, object_value, read_json, text

SCRATCH = Path(__file__).resolve().parents[2] / '.tmp'


class RegistryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        SCRATCH.mkdir(exist_ok=True)
        cls.scratch = tempfile.TemporaryDirectory(prefix='registry-tests.', dir=SCRATCH)
        cls.addClassCleanup(cls.scratch.cleanup)
        cls.root = Path(cls.scratch.name).resolve()
        cls.source = cls.root / 'source'
        cls.source.mkdir()
        files = {
            'Cargo.toml': '[workspace.package]\nversion = "0.1.0"\nlicense = "AGPL-3.0-or-later"\n',
            'apps/garmin-hass/addon/config.yaml': 'version: 0.1.0\nimage: ghcr.io/kubijo/garmin-toolkit-hass\n',
            'flake.lock': '{}\n',
            'assets/licenses/bundle-hass.json': json_bytes(
                {
                    'entries': [
                        {
                            'name': 'synthetic-font',
                            'version': '1',
                            'license': 'MIT',
                            'source': {'kind': 'asset', 'url': 'https://example.invalid/synthetic-font'},
                            'notices': [{'text_index': 0}],
                        }
                    ],
                    'license_texts': ['Synthetic project-owned test notice.'],
                }
            ).decode(),
        }
        for name, content in files.items():
            path = cls.source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        environment = {
            **{key: value for key, value in os.environ.items() if not key.startswith('GIT_')},
            'GIT_CONFIG_NOSYSTEM': '1',
            'GIT_CONFIG_GLOBAL': os.devnull,
            'GIT_AUTHOR_NAME': 'Synthetic',
            'GIT_AUTHOR_EMAIL': 'synthetic@example.invalid',
            'GIT_COMMITTER_NAME': 'Synthetic',
            'GIT_COMMITTER_EMAIL': 'synthetic@example.invalid',
        }
        for command in (['init', '-b', 'main'], ['add', '.'], ['commit', '-m', 'Registry fixture']):
            subprocess.run(
                [
                    'git',
                    '-C',
                    str(cls.source),
                    '-c',
                    'core.hooksPath=/dev/null',
                    '-c',
                    'commit.gpgsign=false',
                    *command,
                ],
                env=environment,
                check=True,
                capture_output=True,
            )
        cls.builds: list[Path] = []
        commit = subprocess.check_output(['git', '-C', str(cls.source), 'rev-parse', 'HEAD'], text=True).strip()
        store = '/nix/store/' + '0' * 32 + '-synthetic'
        for architecture in ('amd64', 'aarch64'):
            directory = cls.root / architecture
            directory.mkdir()
            archive = directory / 'image.oci.tar'
            write_image(
                archive,
                architecture='arm64' if architecture == 'aarch64' else architecture,
                hass_arch=architecture,
                annotations={
                    'org.opencontainers.image.source': 'https://github.com/kubijo/garmin-toolkit',
                    'org.opencontainers.image.revision': commit,
                    'org.opencontainers.image.version': '0.1.0',
                    'org.opencontainers.image.licenses': 'AGPL-3.0-or-later',
                },
                files={
                    store.lstrip('/') + '/bin/garmin-hass': elf(
                        'garmin-hass', 183 if architecture == 'aarch64' else 62
                    ),
                    store.lstrip('/') + '/bin/garmin-gpx-worker': elf(
                        'garmin-gpx-worker', 183 if architecture == 'aarch64' else 62
                    ),
                    store.lstrip('/') + '/share/garmin-hass/web/app.wasm': wasm(),
                },
            )
            closure = directory / 'store-paths'
            closure.write_text(store + '\n')
            sbom = directory / 'image.spdx.json'
            generate(archive, closure, cls.source / 'assets/licenses/bundle-hass.json', sbom, 1)
            system = 'aarch64-linux' if architecture == 'aarch64' else 'x86_64-linux'
            output = directory / 'export'
            with (
                patch('hass_release_build.platform.machine', return_value=system.removesuffix('-linux')),
                patch('hass_release_build.platform.system', return_value='Linux'),
                patch('hass_release_build.native_derivation', return_value=(system, archive)),
            ):
                record_build(
                    NativeBuild(
                        source=cls.source,
                        archive=archive,
                        sbom=sbom,
                        derivation=Path('/nix/store/' + '0' * 32 + '-fixture.drv'),
                        output=output,
                    )
                )
            cls.builds.append(output)
        cls.prepared = cls.root / 'prepared'
        cls.document = prepare_registry(PrepareRegistry(source=cls.source, builds=cls.builds, output=cls.prepared))
        cls.expected = annotations(cls.source, cls.document)

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix='case.', dir=self.root)
        self.addCleanup(temporary.cleanup)
        self.case = Path(temporary.name)
        self.layout = self.case / 'oci'
        shutil.copytree(self.prepared / 'oci', self.layout)
        self.document = copy.deepcopy(type(self).document)

    def rewrite(self, index: Object) -> None:
        descriptor = write_blob(self.layout, index)
        (self.layout / 'index.json').write_bytes(json_bytes({'schemaVersion': 2, 'manifests': [descriptor]}))
        object_value(self.document['image'])['index_digest'] = descriptor['digest']

    def test_annotations_preserve_native_configuration_and_layer_content(self) -> None:
        verify_layout(self.layout, self.document, self.expected)
        self.assertEqual(len(object_rows(self.document['platforms'])), 2)
        for build, platform in zip(self.builds[::-1], object_rows(self.document['platforms']), strict=True):
            native = object_value(object_value(read_json(build / 'native-build.json'))['platform'])
            self.assertEqual(native, platform)

    def test_missing_duplicate_and_substituted_platforms_are_rejected(self) -> None:
        original = metadata(self.layout, root_descriptor(self.layout), INDEX)
        for mutation in ('missing', 'duplicate', 'os', 'architecture', 'revision'):
            with self.subTest(mutation=mutation):
                index = copy.deepcopy(original)
                manifests = object_rows(index['manifests'])
                if mutation == 'missing':
                    index['manifests'] = manifests[:-1]
                elif mutation == 'duplicate':
                    index['manifests'] = [manifests[0], manifests[0]]
                elif mutation == 'revision':
                    object_value(index['annotations'])['org.opencontainers.image.revision'] = '0' * 40
                else:
                    object_value(manifests[0]['platform'])[mutation] = 'substituted'
                self.rewrite(index)
                with self.assertRaises(ValueError):
                    verify_layout(self.layout, self.document, self.expected)

    def test_descriptor_size_digest_and_missing_blobs_are_rejected(self) -> None:
        index = metadata(self.layout, root_descriptor(self.layout), INDEX)
        descriptor = object_rows(index['manifests'])[0]
        blob = self.layout / 'blobs/sha256' / text(descriptor['digest']).removeprefix('sha256:')
        original = blob.read_bytes()
        blob.write_bytes(original + b' ')
        with self.assertRaisesRegex(ValueError, 'size/digest'):
            verify_layout(self.layout, self.document, self.expected)
        blob.unlink()
        with self.assertRaises(FileNotFoundError):
            verify_layout(self.layout, self.document, self.expected)

    def test_substituted_config_external_layers_and_wrong_revision_are_rejected(self) -> None:
        original = metadata(self.layout, root_descriptor(self.layout), INDEX)
        for mutation in ('config', 'layer', 'revision'):
            with self.subTest(mutation=mutation):
                index = copy.deepcopy(original)
                descriptors = object_rows(index['manifests'])
                manifest = metadata(self.layout, descriptors[0], MANIFEST)
                if mutation == 'config':
                    object_value(manifest['config'])['digest'] = 'sha256:' + '0' * 64
                elif mutation == 'layer':
                    object_rows(manifest['layers'])[0]['urls'] = ['https://example.invalid/external']
                else:
                    object_value(manifest['annotations'])['org.opencontainers.image.revision'] = '0' * 40
                descriptors[0] = {**write_blob(self.layout, manifest), 'platform': descriptors[0]['platform']}
                index['manifests'] = descriptors
                self.rewrite(index)
                with self.assertRaises(ValueError):
                    verify_layout(self.layout, self.document, self.expected)

    def test_duplicate_builds_and_existing_outputs_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, 'Duplicate'):
            native_inputs(self.source, [self.builds[0], self.builds[0]])
        with self.assertRaisesRegex(ValueError, 'already exists'):
            prepare_registry(PrepareRegistry(source=self.source, builds=self.builds, output=self.prepared))
        with self.assertRaisesRegex(ValueError, 'outside'):
            prepare_registry(PrepareRegistry(source=self.source, builds=self.builds, output=self.builds[0] / 'nested'))

    def test_oversized_native_archive_is_rejected_before_hashing(self) -> None:
        with (
            patch('archive_io.MAX_IMAGE_BYTES', 100),
            patch('hass_release_build.file_sha256', side_effect=AssertionError('Unexpected payload read')),
            self.assertRaisesRegex(ValueError, 'archive size limit'),
        ):
            native_inputs(self.source, self.builds)

    def test_a_different_filesystem_is_rejected_even_with_valid_descriptor_hashes(self) -> None:
        index = metadata(self.layout, root_descriptor(self.layout), INDEX)
        descriptors = object_rows(index['manifests'])
        manifest = metadata(self.layout, descriptors[0], MANIFEST)
        layer = object_rows(manifest['layers'])[0]
        replacement = gzip.compress(b'Substituted filesystem bytes', mtime=0)
        digest = hashlib.sha256(replacement).hexdigest()
        (self.layout / 'blobs/sha256' / digest).write_bytes(replacement)
        layer.update({'digest': 'sha256:' + digest, 'size': len(replacement)})
        descriptors[0] = {**write_blob(self.layout, manifest), 'platform': descriptors[0]['platform']}
        index['manifests'] = descriptors
        self.rewrite(index)
        with self.assertRaisesRegex(ValueError, 'filesystem|manifest'):
            verify_layout(self.layout, self.document, self.expected)

    def test_failure_leaves_no_partial_output_or_scratch(self) -> None:
        with (
            patch('hass_registry.preflight'),
            patch('hass_registry.unpack_archive', side_effect=ValueError('transport failed')),
            patch('hass_registry.copy_image', side_effect=ValueError('transport failed')),
        ):
            with self.assertRaisesRegex(ValueError, 'transport failed'):
                prepare_registry(PrepareRegistry(source=self.source, builds=self.builds, output=self.case / 'failed'))
            with self.assertRaisesRegex(ValueError, 'transport failed'):
                verify_registry(
                    VerifyRegistry(
                        source=self.source,
                        builds=self.builds,
                        output=self.case / 'failed',
                        reference='ghcr.io/kubijo/garmin-toolkit-hass@sha256:' + '1' * 64,
                    )
                )
        self.assertFalse((self.case / 'failed').exists())
        self.assertFalse(list(self.case.glob('.hass-registry.*')))

    def test_external_urls_are_rejected_before_downloading_any_blobs(self) -> None:
        index = metadata(self.layout, root_descriptor(self.layout), INDEX)
        descriptors = object_rows(index['manifests'])
        manifest = metadata(self.layout, descriptors[0], MANIFEST)
        object_rows(manifest['layers'])[0]['urls'] = ['https://example.invalid/external']
        descriptors[0] = {**write_blob(self.layout, manifest), 'platform': descriptors[0]['platform']}
        index['manifests'] = descriptors
        self.rewrite(index)
        next(platform for platform in object_rows(self.document['platforms']) if platform['architecture'] == 'amd64')[
            'manifest_digest'
        ] = descriptors[0]['digest']
        options = VerifyRegistry(
            source=self.source,
            builds=self.builds,
            output=self.case / 'rejected',
            reference='ghcr.io/kubijo/garmin-toolkit-hass@'
            + text(object_value(self.document['image'])['index_digest']),
        )
        with (
            patch('hass_registry.native_inputs', return_value=self.document),
            patch('hass_registry.remote_manifest', side_effect=[json_bytes(index), json_bytes(manifest)]),
            patch('hass_registry.copy_image') as download,
            self.assertRaisesRegex(ValueError, 'external'),
        ):
            verify_registry(options)
        download.assert_not_called()
        self.assertFalse(options.output.exists())
        self.assertFalse(list(self.case.glob('.hass-registry.*')))

    def test_substituted_native_manifest_is_rejected_before_fetching_it(self) -> None:
        index = metadata(self.layout, root_descriptor(self.layout), INDEX)
        object_rows(index['manifests'])[0]['digest'] = 'sha256:' + '0' * 64
        self.rewrite(index)
        options = VerifyRegistry(
            source=self.source,
            builds=self.builds,
            output=self.case / 'rejected',
            reference='ghcr.io/kubijo/garmin-toolkit-hass@'
            + text(object_value(self.document['image'])['index_digest']),
        )
        with (
            patch('hass_registry.remote_manifest', side_effect=[json_bytes(index), AssertionError('Unexpected fetch')]),
            patch('hass_registry.copy_image') as download,
            self.assertRaisesRegex(ValueError, 'native export'),
        ):
            verify_registry(options)
        download.assert_not_called()

    def test_unpinned_noncanonical_and_insecure_remote_references_are_rejected(self) -> None:
        for reference, plain in (
            ('ghcr.io/kubijo/garmin-toolkit-hass:latest', False),
            ('example.invalid/image@sha256:' + '1' * 64, False),
            ('ghcr.io/kubijo/garmin-toolkit-hass@sha256:' + '1' * 64, True),
        ):
            with self.subTest(reference=reference), self.assertRaisesRegex(ValueError, 'digest-pinned'):
                verify_registry(
                    VerifyRegistry(
                        source=self.source,
                        builds=self.builds,
                        output=self.case / 'result',
                        reference=reference,
                        plain_http=plain,
                    )
                )

    def test_acceptance_is_bound_to_source_and_image_without_claiming_remote_runtime(self) -> None:
        native = object_value(read_json(self.builds[0] / 'native-build.json'))
        platform = object_value(native['platform'])
        installed: Object = {
            'version': native['version'],
            'architecture': 'amd64',
            'config_digest': platform['config_digest'],
            'manifest_digest': platform['manifest_digest'],
            'archive_sha256': platform['archive_sha256'],
            'registry_digest': 'sha256:' + '1' * 64,
        }
        (self.case / 'native-build.json').write_bytes(json_bytes(native))
        (self.case / 'installation.json').write_bytes(json_bytes(installed))
        (self.case / 'workflow.json').write_bytes(json_bytes({'passed': True}))
        result = bind_acceptance((self.case,), self.document)
        self.assertEqual(result[0]['scope'], 'native-image-content')
        self.assertNotEqual(result[0]['runtime_registry_digest'], result[0]['manifest_digest'])
        for field in ('config_digest', 'manifest_digest', 'archive_sha256', 'version', 'architecture'):
            wrong = {**installed, field: 'substituted'}
            (self.case / 'installation.json').write_bytes(json_bytes(wrong))
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'Acceptance'):
                bind_acceptance((self.case,), self.document)
        (self.case / 'installation.json').write_bytes(json_bytes(installed))
        (self.case / 'workflow.json').write_bytes(json_bytes({'passed': False}))
        with self.assertRaisesRegex(ValueError, 'Acceptance'):
            bind_acceptance((self.case,), self.document)
        (self.case / 'workflow.json').write_bytes(json_bytes({'passed': True}))
        object_value(native['source'])['commit'] = '0' * 40
        (self.case / 'native-build.json').write_bytes(json_bytes(native))
        with self.assertRaisesRegex(ValueError, 'source identity'):
            bind_acceptance((self.case,), self.document)

    def round_trip(self, source: Path, builds: list[Path], acceptance: tuple[Path, ...] = ()) -> Object:
        prepared = self.case / 'candidate'
        document = prepare_registry(PrepareRegistry(source=source, builds=builds, output=prepared))
        with registry(self.case) as repository:
            subprocess.run(
                [
                    'skopeo',
                    '--insecure-policy',
                    '--tmpdir',
                    str(self.case),
                    '--command-timeout',
                    '10m',
                    'copy',
                    '--all',
                    '--preserve-digests',
                    '--image-parallel-copies',
                    '1',
                    '--dest-no-creds',
                    '--dest-tls-verify=false',
                    'oci:' + str(prepared / 'oci'),
                    'docker://' + repository + ':candidate',
                ],
                check=True,
                capture_output=True,
                text=True,
                timeout=630,
            )
            result = verify_registry(
                VerifyRegistry(
                    source=source,
                    builds=builds,
                    output=self.case / 'verified',
                    reference=repository + '@' + text(object_value(document['image'])['index_digest']),
                    plain_http=True,
                    acceptance=acceptance,
                )
            )
        self.assertEqual(result['image'], document['image'])
        self.assertEqual(result['platforms'], document['platforms'])
        self.assertEqual(list((self.case / 'verified').iterdir()), [self.case / 'verified/registry.json'])
        self.assertFalse(list(self.case.glob('.hass-registry.*')))
        return result

    def test_real_registry_round_trip_checks_all_platforms(self) -> None:
        self.round_trip(self.source, self.builds)

    @unittest.skipUnless(os.environ.get('GARMIN_REGISTRY_BUILD'), 'Explicit retained native CI export check')
    def test_retained_native_ci_export(self) -> None:
        build = Path(os.environ['GARMIN_REGISTRY_BUILD']).resolve()
        evidence = os.environ.get('GARMIN_REGISTRY_ACCEPTANCE')
        result = self.round_trip(SCRATCH.parent, [build], (Path(evidence).resolve(),) if evidence else ())
        output = os.environ.get('GARMIN_REGISTRY_EVIDENCE')
        if output:
            with Path(output).open('xb') as target:
                target.write(json_bytes(result))


if __name__ == '__main__':
    unittest.main()
