"""License generator tests."""

import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import patch

from generate import bundle_document, expanded_entries, generate, main, resolve_linked


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
        metadata = {
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
            patch('generate.cargo_metadata', side_effect=lambda triple: triple),
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
        entry = {
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
