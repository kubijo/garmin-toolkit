"""License generator tests."""

import hashlib
import io
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from typing import Any
from unittest.mock import patch

from generate import (
    bundle_document,
    expanded_entries,
    generate,
    harvested_entries,
    main,
    notice_supplements,
    package_file,
    package_notices,
    resolve_linked,
)


class LicenseGenerationTests(unittest.TestCase):
    """Exercise graph selection and bundle encoding."""

    def test_subprocess_diagnostics_reach_stderr(self) -> None:
        result = subprocess.run(
            [
                sys.executable,
                '-c',
                """
from generate import run_json
import sys
run_json([sys.executable, '-c', 'import sys; print("Cargo diagnostic", file=sys.stderr); sys.exit(101)'])
""",
            ],
            cwd=Path(__file__).parent,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Cargo diagnostic', result.stderr.splitlines())

    def test_cli_reports_failed_command_without_traceback(self) -> None:
        for arguments in ([], ['--check']):
            with self.subTest(arguments=arguments), tempfile.TemporaryDirectory() as directory:
                config = Path(directory) / 'config.json'
                config.write_text('{}')
                error = subprocess.CalledProcessError(101, ['cargo', 'metadata', '--locked'])
                stderr = io.StringIO()
                with (
                    patch.object(sys, 'argv', ['generate.py', '--config', str(config), *arguments]),
                    patch('generate.generate', side_effect=error),
                    redirect_stderr(stderr),
                ):
                    self.assertEqual(main(), 1)
                self.assertEqual(stderr.getvalue(), 'license generation failed: cargo metadata --locked (exit 101)\n')

    def test_resolves_only_normal_non_macro_dependencies(self) -> None:
        packages = [
            self.package('root', None, ['bin']),
            self.package('normal', 'registry', ['lib']),
            self.package('build', 'registry', ['lib']),
            self.package('macro', 'registry', ['proc-macro']),
        ]
        metadata: dict[str, Any] = {
            'packages': packages,
            'resolve': {
                'nodes': [
                    {
                        'deps': [
                            self.dependency('normal', None),
                            self.dependency('build', 'build'),
                            self.dependency('macro', None),
                        ],
                        'id': 'root',
                    },
                    {'deps': [], 'id': 'normal'},
                    {'deps': [], 'id': 'build'},
                    {'deps': [], 'id': 'macro'},
                ]
            },
        }

        self.assertEqual(
            resolve_linked(metadata, 'root'),
            {('normal', '1'): True, ('root', '1'): False},
        )

    def test_bundle_deduplicates_text(self) -> None:
        entries = [
            {
                'license': 'MIT',
                'name': name,
                'notices': [{'license': 'MIT', 'text': 'same'}],
                'source': {'kind': 'cargo'},
                'version': '1',
            }
            for name in ('b', 'a')
        ]

        bundle = json.loads(bundle_document(entries))

        self.assertEqual(bundle['schema_version'], 1)
        self.assertEqual(bundle['license_texts'], ['same'])
        self.assertEqual([entry['name'] for entry in bundle['entries']], ['a', 'b'])
        self.assertTrue(all(entry['notices'][0]['text_index'] == 0 for entry in bundle['entries']))

    def test_every_declared_notice_requires_real_text(self) -> None:
        for invalid in ('', ' \n ', 'NOT FOUND', ' NOT FOUND\n'):
            with self.subTest(text=invalid), self.assertRaisesRegex(ValueError, 'example 1.*Apache-2.0'):
                bundle_document(
                    [
                        {
                            'name': 'example',
                            'version': '1',
                            'license': 'MIT OR Apache-2.0',
                            'notices': [
                                {'license': 'MIT', 'text': 'valid MIT notice'},
                                {'license': 'Apache-2.0', 'text': invalid},
                            ],
                        }
                    ]
                )
        with self.assertRaisesRegex(ValueError, 'no notices'):
            bundle_document([{'name': 'example', 'version': '1', 'notices': []}])

    def test_notice_paths_cannot_escape_package_through_parent_or_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'LICENSE').write_text('unrelated parent notice')
            package = root / 'dependency'
            package.mkdir()
            (package / 'LICENSE').symlink_to(root / 'LICENSE')
            metadata = {'name': 'dependency', 'manifest_path': str(package / 'Cargo.toml')}
            self.assertEqual(package_notices(metadata), set())
            for path in ('../LICENSE', 'LICENSE'):
                with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'escapes dependency'):
                    package_file(metadata, path)

    def test_package_notice_bytes_preserve_crlf(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'LICENSE').write_bytes(b'Copyright upstream\r\nTerms\r\n')
            self.assertEqual(
                package_notices({'manifest_path': str(root / 'Cargo.toml')}),
                {'Copyright upstream\r\nTerms\r\n'},
            )

    def test_upstream_notice_requires_version_checksum_and_pinned_source(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            text = b'Actual upstream notice\n'
            (root / 'LICENSE.txt').write_bytes(text)
            manifest = root / 'notices.toml'
            source = 'https://example.com/project/' + 'a' * 40 + '/LICENSE'
            digest = hashlib.sha256(text).hexdigest()
            valid = (
                '[[notices]]\npackages = ["dependency 1"]\nlicense = "MIT"\n'
                f'file = "LICENSE.txt"\nsource = "{source}"\nsha256 = "{digest}"\n'
            )
            manifest.write_text(valid)
            packages: dict[tuple[str, str], dict[str, Any]] = {('dependency', '1'): {}}
            self.assertEqual(notice_supplements(packages, manifest), {('dependency', '1', 'MIT'): text.decode()})
            manifest.write_text(
                valid.replace(digest, hashlib.sha256(text[:-1]).hexdigest()) + 'no_final_newline = true\n'
            )
            self.assertEqual(notice_supplements(packages, manifest), {('dependency', '1', 'MIT'): text[:-1].decode()})
            manifest.write_text(valid)
            with self.assertRaisesRegex(ValueError, 'stale license supplement'):
                notice_supplements({('dependency', '2'): {}}, manifest)
            (root / 'LICENSE.txt').write_text('modified notice')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                notice_supplements(packages, manifest)
            manifest.write_text(valid.replace('a' * 40, 'main'))
            with self.assertRaisesRegex(ValueError, 'must pin a revision'):
                notice_supplements(packages, manifest)

    def test_reference_notice_keeps_copyright_and_includes_full_standard_terms(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = 'Copyright actual upstream author\nLicensed under MIT; see https://example.com/MIT\n'
            terms = 'Permission is hereby granted, subject to these conditions.\n'
            (root / 'NOTICE.txt').write_text(original)
            (root / 'MIT.txt').write_text(terms)
            manifest = root / 'notices.toml'
            source = 'https://example.com/project/' + 'a' * 40 + '/LICENSE'
            terms_source = 'https://raw.githubusercontent.com/spdx/license-list-data/' + 'b' * 40 + '/text/MIT.txt'
            configured = (
                '[[notices]]\npackages = ["dependency 1"]\nlicense = "MIT"\n'
                f'file = "NOTICE.txt"\nsource = "{source}"\n'
                f'sha256 = "{hashlib.sha256(original.encode()).hexdigest()}"\n'
                'include_standard_terms = true\n'
                f'[terms.MIT]\nfile = "MIT.txt"\nsource = "{terms_source}"\n'
                f'sha256 = "{hashlib.sha256(terms.encode()).hexdigest()}"\n'
            )
            manifest.write_text(configured)
            packages: dict[tuple[str, str], dict[str, Any]] = {('dependency', '1'): {}}
            text = notice_supplements(packages, manifest)[('dependency', '1', 'MIT')]
            self.assertTrue(text.startswith(original))
            self.assertIn(terms, text)
            self.assertIn(terms_source, text)
            manifest.write_text(configured.replace('include_standard_terms = true', 'include_standard_terms = false'))
            self.assertEqual(notice_supplements(packages, manifest)[('dependency', '1', 'MIT')], original)

    def test_failed_target_does_not_replace_other_bundles(self) -> None:
        config = {
            'assets': [],
            'targets': [{'name': name, 'package': name, 'triples': ['native']} for name in ('desktop', 'hass')],
        }
        with (
            tempfile.TemporaryDirectory() as directory,
            patch('generate.harvested_entries', return_value={}),
            patch('generate.cargo_metadata', return_value={}),
            patch('generate.resolve_linked', side_effect=[{}, {('missing', '1'): True}]),
        ):
            root = Path(directory)
            (root / 'bundle-desktop.json').write_text('previous desktop bundle')
            (root / 'bundle-stale.json').write_text('previous stale bundle')
            with self.assertRaisesRegex(ValueError, 'omitted'):
                generate(config, root)
            self.assertEqual((root / 'bundle-desktop.json').read_text(), 'previous desktop bundle')
            self.assertEqual((root / 'bundle-stale.json').read_text(), 'previous stale bundle')

    @unittest.skipUnless(shutil.which('cargo') and shutil.which('cargo-bundle-licenses'), 'requires pinned Cargo tools')
    def test_real_harvester_rejects_parent_notice_and_matches_both_cache_layouts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            documents: list[str] = []
            for layout in ('checkout/.tmp/registry', 'vendor'):
                dependency = root / layout / 'dependency'
                dependency.mkdir(parents=True)
                (dependency / 'Cargo.toml').write_text(
                    '[workspace]\n[package]\nname = "dependency"\nversion = "1.0.0"\nlicense = "MIT"\n'
                )
                (dependency / 'src').mkdir()
                (dependency / 'src/lib.rs').write_text('')
                checkout = root / 'checkout'
                checkout.mkdir(exist_ok=True)
                (checkout / 'LICENSE-MIT').write_text('Copyright this application, NOT the dependency')
                (checkout / 'src').mkdir(exist_ok=True)
                (checkout / 'src/lib.rs').write_text('')
                (checkout / 'Cargo.toml').write_text(
                    '[workspace]\nexclude = [".tmp/registry/dependency"]\n'
                    '[package]\nname = "application"\nversion = "1.0.0"\n'
                    f'[dependencies]\ndependency = {{ path = "{dependency}" }}\n'
                )
                # Metadata is locked in production; resolve this disposable fixture first.
                resolved = subprocess.run(
                    ['cargo', 'generate-lockfile', '--offline'],
                    cwd=checkout,
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(resolved.returncode, 0, resolved.stderr)
                with (
                    patch('generate.REPOSITORY', checkout),
                    patch('generate.notice_supplements', return_value={}),
                ):
                    entries = harvested_entries()
                    self.assertEqual(entries[('dependency', '1.0.0')]['notices'][0]['text'], '')
                with (
                    patch('generate.REPOSITORY', checkout),
                    patch(
                        'generate.notice_supplements',
                        return_value={('dependency', '1.0.0', 'MIT'): 'Verified upstream dependency notice'},
                    ) as supplements,
                ):
                    documents.append(bundle_document(list(harvested_entries().values())))
                    (dependency / 'LICENSE-MIT').write_text('Copyright actual upstream contributor\nTerms\n')
                    retained = harvested_entries()[('dependency', '1.0.0')]['notices'][0]['text']
                    self.assertEqual(retained, 'Copyright actual upstream contributor\nTerms\n')
                    extended = retained + '\nComplete standard terms\n'
                    supplements.return_value = {('dependency', '1.0.0', 'MIT'): extended}
                    self.assertEqual(harvested_entries()[('dependency', '1.0.0')]['notices'][0]['text'], extended)
            self.assertEqual(documents[0], documents[1])

    def test_bundle_includes_shipped_web_client_dependencies(self) -> None:
        config = {
            'assets': [],
            'targets': [
                {
                    'name': 'hass',
                    'package': 'server',
                    'triples': ['native'],
                    'bundled': [{'package': 'web', 'triples': ['wasm']}],
                }
            ],
        }
        cargo = {
            (name, '1'): {
                'name': name,
                'version': '1',
                'license': 'MIT',
                'notices': [{'license': 'MIT', 'text': name}],
                'source': {'kind': 'cargo'},
            }
            for name in ('server-dep', 'picker')
        }
        with (
            tempfile.TemporaryDirectory() as directory,
            patch('generate.harvested_entries', return_value=cargo),
            patch('generate.cargo_metadata', side_effect=str),
            patch(
                'generate.resolve_linked', side_effect=[{('server-dep', '1'): True}, {('picker', '1'): True}]
            ) as resolve,
        ):
            output = Path(directory)
            generate(config, output)
            self.assertEqual(resolve.call_args_list[0].args, ('native', 'server'))
            self.assertEqual(resolve.call_args_list[1].args, ('wasm', 'web'))
            self.assertEqual(set(expanded_entries(output / 'bundle-hass.json')), {'server-dep 1', 'picker 1'})

    @staticmethod
    def package(name: str, source: str | None, kinds: list[str]) -> dict[str, object]:
        """Create a Cargo metadata package fixture."""
        return {
            'id': name,
            'name': name,
            'source': source,
            'targets': [{'kind': kinds}],
            'version': '1',
        }

    def test_notice_diagnostics_compare_text_not_table_indices(self) -> None:
        entry: dict[str, Any] = {
            'name': 'example',
            'version': '1',
            'license': 'MIT',
            'source': {'kind': 'cargo'},
            'notices': [{'license': 'MIT', 'text_index': 0}],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'bundle.json'
            path.write_text(json.dumps({'entries': [entry], 'license_texts': ['notice']}))
            before = expanded_entries(path)
            entry['notices'][0]['text_index'] = 1
            path.write_text(json.dumps({'entries': [entry], 'license_texts': ['unrelated', 'notice']}))
            self.assertEqual(before, expanded_entries(path))
            path.write_text(json.dumps({'entries': [entry], 'license_texts': ['unrelated', 'changed']}))
            self.assertNotEqual(before, expanded_entries(path))

    @staticmethod
    def dependency(package: str, kind: str | None) -> dict[str, object]:
        """Create a Cargo metadata dependency fixture."""
        return {'dep_kinds': [{'kind': kind}], 'pkg': package}


if __name__ == '__main__':
    unittest.main()
