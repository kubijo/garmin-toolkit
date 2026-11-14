"""Exercise release inventories, architecture channels, and immutable publication using tiny ELF fixtures."""

import hashlib
import json
import os
import struct
import subprocess
import tempfile
import unittest
from contextlib import chdir
from pathlib import Path
from unittest.mock import patch

import desktop_release as release
import desktop_sources
from desktop_source_fixtures import STORE_PATH, source_assets
from github_release_fixtures import FIXTURE_COMMIT, ReleaseFixture
from github_releases import ReleaseAsset
from json_data import object_rows, object_value, read_json, text

TAG = 'v0.1.5'
SOURCE = release.ReleaseSource(TAG, FIXTURE_COMMIT)


def appimage(path: Path, architecture: str, info: str) -> None:
    header = bytearray(64)
    header[:6] = b'\x7fELF\x02\x01'
    header[8:11] = b'AI\x02'
    struct.pack_into('<H', header, 18, {'x86_64': 62, 'aarch64': 183}[architecture])
    struct.pack_into('<I', header, 20, 1)
    struct.pack_into('<H', header, 52, 64)
    struct.pack_into('<Q', header, 40, 64)
    struct.pack_into('<HHH', header, 58, 64, 3, 1)
    names = b'\0.shstrtab\0.upd_info\0'
    strings_offset = 64 + 3 * 64
    table = bytearray(3 * 64)
    struct.pack_into('<I', table, 64, 1)
    struct.pack_into('<I', table, 64 + 4, 3)
    struct.pack_into('<QQ', table, 64 + 24, strings_offset, len(names))
    struct.pack_into('<I', table, 128, 11)
    struct.pack_into('<I', table, 128 + 4, 1)
    struct.pack_into('<QQ', table, 128 + 24, strings_offset + len(names), 1024)
    path.write_bytes(header + table + names + info.encode().ljust(1024, b'\0'))


def assets(directory: Path) -> None:
    for architecture in release.ARCHITECTURES:
        for mode in release.MODES:
            name = f'{mode}-{architecture}.AppImage'
            image = directory / name
            appimage(image, architecture, f'gh-releases-zsync|kubijo|garmin-toolkit|latest-all|{name}.zsync')
            subprocess.run(
                [
                    'zsyncmake',
                    '-f',
                    name,
                    '-u',
                    f'https://github.com/{release.REPOSITORY}/releases/download/{TAG}/{name}',
                    '-o',
                    str(directory / (name + '.zsync')),
                    str(image),
                ],
                check=True,
                capture_output=True,
            )
    source_assets(directory, TAG, FIXTURE_COMMIT)
    paths = release.validate_assets(directory, TAG)
    manifests = [
        {
            'schema_version': 1,
            'tag': TAG,
            'commit': FIXTURE_COMMIT,
            'system': system,
            'assets': {
                name: release.digest(directory / name) for name in sorted(release.native_asset_names(system, directory))
            },
        }
        for system in release.SYSTEMS
    ]
    (directory / release.STAGED_MANIFEST).write_text(json.dumps(manifests))
    (directory / release.CHECKSUMS).write_text(release.checksums(paths))


def native_builds(inputs: Path) -> Path:
    fixtures = inputs.parent / 'fixtures'
    fixtures.mkdir()
    assets(fixtures)
    manifests = object_rows(read_json(fixtures / release.STAGED_MANIFEST))
    for manifest in manifests:
        native = inputs / text(manifest['system'])
        native.mkdir(parents=True)
        for name in object_value(manifest['assets']):
            (native / name).write_bytes((fixtures / name).read_bytes())
        (native / release.NATIVE_MANIFEST).write_text(json.dumps(manifest))
    return fixtures


def git(directory: Path, *arguments: str) -> str:
    return subprocess.run(
        [
            'git',
            '-C',
            str(directory),
            '-c',
            'user.name=Release fixture',
            '-c',
            'user.email=fixture@example.invalid',
            *arguments,
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def source_checkout(directory: Path) -> release.ReleaseSource:
    directory.mkdir()
    git(directory, 'init', '-q', '--initial-branch=main')
    (directory / '.gitignore').write_text('.tmp/\n')
    (directory / 'Cargo.toml').write_text('[workspace.package]\nversion="0.1.5"\n')
    (directory / 'main.rs').write_text('fn main() { println!("build A"); }\n')
    git(directory, 'add', '.')
    git(directory, 'commit', '-qm', 'Synthetic source A')
    git(directory, 'tag', TAG)
    return release.ReleaseSource(TAG, git(directory, 'rev-parse', 'HEAD'))


class DesktopReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.enterContext(patch.object(desktop_sources, 'measured_store_paths', return_value={STORE_PATH}))
        self.enterContext(patch.object(desktop_sources, 'app_run_hash', return_value='a' * 64))

    def test_tampered_dependency_sources_fail_before_github_access(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            archive = root / desktop_sources.names('aarch64')[2]
            with archive.open('ab') as stream:
                stream.write(b'synthetic corruption')
            client = ReleaseFixture(directory=root, draft=False)
            with patch.object(release, 'source_identity'), self.assertRaisesRegex(ValueError, 'source part'):
                release.publish(SOURCE, root, releases=client)
            self.assertEqual(client.tag_lookups, [])
            self.assertEqual(client.uploads, [])

    def test_source_resolution_pins_the_trigger_and_rejects_later_local_tag_moves(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            checkout = Path(temporary) / 'checkout'
            source = source_checkout(checkout)
            client = ReleaseFixture(directory=checkout, draft=False, commit=source.commit)
            with chdir(checkout):
                self.assertEqual(release.resolve_source(TAG, source.commit, releases=client), source)
            (checkout / 'main.rs').write_text('fn main() { println!("build B"); }\n')
            git(checkout, 'commit', '-qam', 'Synthetic source B')
            git(checkout, 'tag', '-f', TAG)
            with chdir(checkout):
                with self.assertRaisesRegex(ValueError, 'pinned release commit'):
                    release.source_identity(source)
                with self.assertRaisesRegex(ValueError, 'triggering commit'):
                    release.resolve_source(TAG, source.commit, releases=client)
            git(checkout, 'checkout', '-q', '--detach', source.commit)
            with chdir(checkout), self.assertRaisesRegex(ValueError, 'release tag'):
                release.source_identity(source)

    def test_staging_rejects_artifacts_built_before_a_tag_move(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = root / 'inputs'
            native_builds(inputs)
            newer = release.ReleaseSource(TAG, '2' * 40)
            with patch.object(release, 'source_identity'), self.assertRaisesRegex(ValueError, 'pinned source'):
                release.stage(newer, inputs, root / 'publish')

    def test_publication_rejects_staged_artifacts_from_another_commit_before_network_access(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            client = ReleaseFixture(directory=root, draft=False, commit='2' * 40)
            newer = release.ReleaseSource(TAG, client.commit)
            with patch.object(release, 'source_identity'), self.assertRaisesRegex(ValueError, 'pinned source'):
                release.publish(newer, root, releases=client)
            self.assertEqual(client.tag_lookups, [])
            self.assertEqual(client.uploads, [])

    def test_publication_rechecks_the_remote_tag_after_existing_asset_inspection(self) -> None:
        class MovingTag(ReleaseFixture):
            def assets(self, tag: str) -> list[ReleaseAsset]:
                self.commit = '2' * 40
                return super().assets(tag)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            client = MovingTag(directory=root, draft=False)
            with patch.object(release, 'source_identity'), self.assertRaisesRegex(ValueError, 'Remote release tag'):
                release.publish(SOURCE, root, releases=client)
            self.assertEqual(client.tag_lookups, [TAG, TAG])
            self.assertEqual(client.uploads, [])

    def test_native_build_records_the_pinned_source_and_exact_output_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = root / 'inputs'
            fixtures = native_builds(inputs)
            system = 'x86_64-linux'
            store = root / 'store'
            store.mkdir()
            for name in release.native_image_names(system):
                (store / name).write_bytes((inputs / system / name).read_bytes())
            output = root / 'native'
            output.with_name(output.name + '.nix').symlink_to(store)

            def export(directory: Path, system: str, tag: str, commit: str) -> None:
                for name in desktop_sources.asset_names(fixtures, release.SYSTEMS[system]):
                    (directory / name).write_bytes((fixtures / name).read_bytes())

            with (
                patch.object(release, 'source_identity'),
                patch.object(release.subprocess, 'run') as build,
                patch.object(desktop_sources, 'export', side_effect=export),
            ):
                release.build(SOURCE, system, output)
            build.assert_called_once()
            manifest = object_value(read_json(output / release.NATIVE_MANIFEST))
            self.assertEqual(manifest['commit'], SOURCE.commit)
            self.assertEqual(release.verify_build_manifest(manifest, output, SOURCE), system)
            self.assertFalse(output.is_symlink())

    def test_native_asset_hash_changes_are_rejected_during_staging(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = root / 'inputs'
            native_builds(inputs)
            image = inputs / 'aarch64-linux' / 'garmin-toolkit-aarch64.AppImage'
            with image.open('ab') as changed:
                changed.write(b'synthetic artifact alteration')
            with patch.object(release, 'source_identity'), self.assertRaisesRegex(ValueError, 'native build'):
                release.stage(SOURCE, inputs, root / 'publish')

    def test_corrupt_or_missing_control_blocks_fail_before_github_access(self) -> None:
        for truncate in [True, False]:
            with self.subTest(truncate=truncate), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                assets(root)
                control = root / 'garmin-toolkit-x86_64.AppImage.zsync'
                header, blocks = control.read_bytes().split(b'\n\n', 1)
                self.assertTrue(blocks)
                corrupt = b'' if truncate else bytes([blocks[0] ^ 0xFF]) + blocks[1:]
                control.write_bytes(header + b'\n\n' + corrupt)
                client = ReleaseFixture(directory=root, draft=False)
                with patch.object(release, 'source_identity'), self.assertRaisesRegex(ValueError, 'zsync metadata'):
                    release.publish(SOURCE, root, releases=client)
                self.assertEqual(client.lookups, [])
                self.assertEqual(client.uploads, [])

    def test_artifact_transport_mtime_changes_do_not_invalidate_controls(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            for image in root.glob('*.AppImage'):
                os.utime(image, (1, 1))
            self.assertEqual(len(release.validate_assets(root, TAG)), 14)

    def test_workspace_version_mismatch_fails_before_git_or_nix(self) -> None:
        with patch.object(release, 'command') as command, self.assertRaisesRegex(ValueError, 'workspace version'):
            release.source_identity(release.ReleaseSource('v0.0.0-synthetic-mismatch', FIXTURE_COMMIT))
        command.assert_not_called()

    def test_staging_combines_both_native_builds_and_generates_the_final_inventory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = root / 'inputs'
            fixtures = native_builds(inputs)
            output = root / 'publish'
            with patch.object(release, 'source_identity'):
                release.stage(SOURCE, inputs, output)
            self.assertEqual((output / release.CHECKSUMS).read_bytes(), (fixtures / release.CHECKSUMS).read_bytes())

    def test_all_mode_and_architecture_channels_validate_without_executing_the_appimages(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            self.assertEqual(len(release.validate_assets(root, TAG)), 14)

    def test_wrong_architecture_and_missing_update_metadata_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            image = Path(temporary) / 'test.AppImage'
            appimage(image, 'aarch64', 'synthetic')
            with self.assertRaisesRegex(ValueError, 'Invalid x86_64'):
                release.update_information(image, 'x86_64')
            image.write_bytes(b'not an AppImage')
            with self.assertRaises(ValueError):
                release.update_information(image, 'aarch64')

    def test_control_file_must_hash_final_bytes_and_use_the_matching_release_url(self) -> None:
        for field in ['SHA-1', 'URL']:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                assets(root)
                control = root / 'garmin-toolkit-aarch64.AppImage.zsync'
                header, body = control.read_bytes().split(b'\n\n', 1)
                rows = header.decode().splitlines()
                control.write_bytes(
                    '\n'.join(f'{field}: wrong' if row.startswith(field + ':') else row for row in rows).encode()
                    + b'\n\n'
                    + body
                )
                with self.assertRaisesRegex(ValueError, 'zsync metadata'):
                    release.validate_assets(root, TAG)

    def test_missing_architecture_or_unexpected_assets_fail_before_upload(self) -> None:
        for remove in [True, False]:
            with self.subTest(remove=remove), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                assets(root)
                if remove:
                    (root / 'garmin-toolkit-aarch64.AppImage').unlink()
                else:
                    (root / 'surprise.zip').touch()
                releases = ReleaseFixture(directory=root, draft=False)
                with (
                    patch.object(release, 'source_identity'),
                    self.assertRaisesRegex(ValueError, 'inventory differs'),
                ):
                    release.publish(SOURCE, root, releases=releases)
                self.assertEqual(releases.lookups, [])
                self.assertEqual(releases.uploads, [])

    def test_a_different_channel_cannot_be_published(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            image = root / 'garmin-toolkit-aarch64.AppImage'
            appimage(image, 'aarch64', 'gh-releases-zsync|other|repository|latest|wrong.zsync')
            with self.assertRaisesRegex(ValueError, 'Wrong update channel'):
                release.validate_assets(root, TAG)

    def test_rerun_skips_identical_assets_and_uploads_only_missing_ones(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            present = root / 'garmin-toolkit-x86_64.AppImage'
            releases = ReleaseFixture(
                directory=root,
                draft=False,
                existing_assets=[ReleaseAsset(present.name, 'sha256:' + release.digest(present))],
            )
            with patch.object(release, 'source_identity'):
                release.publish(SOURCE, root, releases=releases)
                release.publish(SOURCE, root, releases=releases)
            self.assertEqual(len(releases.uploads), 1)
            upload = releases.uploads[0]
            self.assertEqual(upload.tag, TAG)
            self.assertNotIn(present, upload.paths)
            self.assertIn(root / release.CHECKSUMS, upload.paths)

    def test_legacy_assets_without_server_digests_are_compared_using_a_typed_download(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            present = root / 'garmin-toolkit-x86_64.AppImage'
            releases = ReleaseFixture(directory=root, draft=False, existing_assets=[ReleaseAsset(present.name, None)])
            with patch.object(release, 'source_identity'):
                release.publish(SOURCE, root, releases=releases)
            self.assertEqual(len(releases.downloads), 1)
            self.assertEqual(releases.downloads[0].pattern, present.name)
            self.assertNotIn(present, releases.uploads[0].paths)

    def test_conflicting_published_bytes_are_rejected_before_any_upload(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            releases = ReleaseFixture(
                directory=root,
                draft=False,
                existing_assets=[ReleaseAsset('garmin-toolkit-x86_64.AppImage', 'sha256:' + '0' * 64)],
            )
            with (
                patch.object(release, 'source_identity'),
                self.assertRaisesRegex(ValueError, 'Refusing to replace'),
            ):
                release.publish(SOURCE, root, releases=releases)
            self.assertEqual(releases.uploads, [])

    def test_assets_are_not_attached_to_the_hass_draft(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            releases = ReleaseFixture(directory=root, draft=True)
            with (
                patch.object(release, 'source_identity'),
                self.assertRaisesRegex(ValueError, 'published release'),
            ):
                release.publish(SOURCE, root, releases=releases)
            self.assertEqual(releases.lookups, [TAG])
            self.assertEqual(releases.uploads, [])

    def test_checksums_use_sha256_and_deterministic_asset_names(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets(root)
            lines = (root / release.CHECKSUMS).read_text().splitlines()
            self.assertEqual([line.split('  ')[1] for line in lines], sorted(release.asset_names(root)))
            for line in lines:
                expected, name = line.split('  ')
                self.assertEqual(expected, hashlib.sha256((root / name).read_bytes()).hexdigest())


if __name__ == '__main__':
    unittest.main()
