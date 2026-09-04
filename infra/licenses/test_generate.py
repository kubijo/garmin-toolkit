"""License generator tests."""

import json
import unittest

from generate import bundle_document, resolve_linked


class LicenseGenerationTests(unittest.TestCase):
    """Exercise graph selection and bundle encoding."""

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

    @staticmethod
    def dependency(package: str, kind: str | None) -> dict[str, object]:
        """Create a Cargo metadata dependency fixture."""
        return {'dep_kinds': [{'kind': kind}], 'pkg': package}


if __name__ == '__main__':
    unittest.main()
