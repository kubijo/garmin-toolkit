"""Declaration fallback provenance and failure regressions."""

import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

from declarations import declaration_supplements


class DeclarationTests(unittest.TestCase):
    """Only reviewed, unchanged package declarations receive standard terms."""

    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.package_root = self.root / 'dependency'
        self.package_root.mkdir()
        self.original = (
            '[package]\nname = "dependency"\nversion = "1.0.0"\nlicense = "MIT"\nauthors = ["Upstream Author"]\n'
        )
        (self.package_root / 'Cargo.toml.orig').write_text(self.original)
        (self.package_root / '.cargo_vcs_info.json').write_text(
            json.dumps({'git': {'sha1': 'a' * 40}, 'path_in_vcs': 'crates/dependency'})
        )
        self.package: dict[str, Any] = {
            'name': 'dependency',
            'version': '1.0.0',
            'license': 'MIT',
            'authors': ['Upstream Author'],
            'repository': 'https://github.com/upstream/project',
            'manifest_path': str(self.package_root / 'Cargo.toml'),
        }
        self.template = 'MIT License\n\nCopyright (c) <year> <copyright holders>\n\nPermission notice\n'
        (self.root / 'MIT.txt').write_text(self.template)
        self.declaration: dict[str, Any] = {
            'package': 'dependency 1.0.0',
            'expression': 'MIT',
            'licenses': ['MIT'],
            'source': 'https://raw.githubusercontent.com/upstream/project/'
            + 'a' * 40
            + '/crates/dependency/Cargo.toml',
            'sha256': hashlib.sha256(self.original.encode()).hexdigest(),
        }
        self.config: dict[str, Any] = {
            'declarations': [self.declaration],
            'terms': {
                'MIT': {
                    'file': 'MIT.txt',
                    'source': 'https://raw.githubusercontent.com/spdx/license-list-data/' + 'b' * 40 + '/text/MIT.txt',
                    'sha256': hashlib.sha256(self.template.encode()).hexdigest(),
                    'omit_template_line': 'Copyright (c) <year> <copyright holders>',
                }
            },
        }

    def generate(self) -> dict[tuple[str, str, str], str]:
        return declaration_supplements({('dependency', '1.0.0'): self.package}, self.config, self.root)

    def test_labels_standard_terms_and_preserves_actual_author_attribution(self) -> None:
        notice = self.generate()[('dependency', '1.0.0', 'MIT')]
        self.assertIn('Authors (Cargo metadata): Upstream Author\n', notice)
        self.assertIn(f'Declaration: {self.declaration["source"]}\n', notice)
        self.assertIn('Standard license terms (MIT): ', notice)
        self.assertIn('Permission notice\n', notice)
        self.assertNotIn('<year>', notice)
        self.assertNotIn('<copyright holders>', notice)
        self.assertNotIn('Copyright (c) Upstream Author', notice)

    def test_does_not_automatically_cover_unreviewed_packages(self) -> None:
        self.config['declarations'] = []
        self.assertEqual(self.generate(), {})
        self.config['declarations'] = [{**self.declaration, 'package': 'dependency 2.0.0'}]
        with self.assertRaisesRegex(ValueError, 'stale license declaration'):
            self.generate()

    def test_modified_original_manifest_is_rejected(self) -> None:
        (self.package_root / 'Cargo.toml.orig').write_text(self.original + '\n')
        with self.assertRaisesRegex(ValueError, 'declaration checksum mismatch'):
            self.generate()

    def test_published_metadata_and_reviewed_expression_must_match(self) -> None:
        for field, value in (('license', 'Apache-2.0'), ('authors', ['Different Author'])):
            with self.subTest(field=field):
                original = self.package[field]
                self.package[field] = value
                with self.assertRaisesRegex(ValueError, 'metadata mismatch'):
                    self.generate()
                self.package[field] = original
        self.declaration['expression'] = 'Apache-2.0'
        with self.assertRaisesRegex(ValueError, 'expression changed'):
            self.generate()

    def test_source_must_match_package_repository_revision_and_subdirectory(self) -> None:
        for source in (
            self.declaration['source'].replace('a' * 40, 'c' * 40),
            self.declaration['source'].replace('upstream/project', 'unrelated/project'),
            self.declaration['source'].replace('crates/dependency/', ''),
        ):
            with self.subTest(source=source):
                configured = self.declaration['source']
                self.declaration['source'] = source
                with self.assertRaisesRegex(ValueError, 'origin mismatch'):
                    self.generate()
                self.declaration['source'] = configured

    def test_terms_checksum_and_template_adjustment_fail_closed(self) -> None:
        (self.root / 'MIT.txt').write_text('modified terms')
        with self.assertRaisesRegex(ValueError, 'license checksum mismatch'):
            self.generate()
        (self.root / 'MIT.txt').write_text(self.template)
        self.config['terms']['MIT']['omit_template_line'] = 'Copyright (c) <year> <different holders>'
        with self.assertRaisesRegex(ValueError, 'template line changed'):
            self.generate()

    def test_template_adjustment_cannot_remove_a_license_condition(self) -> None:
        self.config['terms']['MIT']['omit_template_line'] = 'Permission notice'
        with self.assertRaisesRegex(ValueError, 'only copyright examples may be omitted'):
            self.generate()

    def test_all_or_alternatives_are_retained_and_and_is_not_flattened(self) -> None:
        for expression, licenses in (
            ('MIT OR Apache-2.0', ['MIT', 'Apache-2.0']),
            ('MIT AND Apache-2.0', ['MIT AND Apache-2.0']),
        ):
            with self.subTest(expression=expression):
                original = self.original.replace('license = "MIT"', f'license = "{expression}"')
                (self.package_root / 'Cargo.toml.orig').write_text(original)
                self.package['license'] = expression
                self.declaration.update(
                    expression=expression, licenses=licenses, sha256=hashlib.sha256(original.encode()).hexdigest()
                )
                self.config['terms']['Apache-2.0'] = copy.deepcopy(self.config['terms']['MIT'])
                self.config['terms']['Apache-2.0']['source'] = self.config['terms']['MIT']['source'].replace(
                    '/MIT.txt', '/Apache-2.0.txt'
                )
                if ' AND ' in expression:
                    with self.assertRaisesRegex(ValueError, 'unreviewed standard license terms'):
                        self.generate()
                else:
                    self.assertEqual(
                        set(self.generate()),
                        {('dependency', '1.0.0', 'MIT'), ('dependency', '1.0.0', 'Apache-2.0')},
                    )
        self.declaration['licenses'] = ['MIT']
        with self.assertRaisesRegex(ValueError, 'alternatives mismatch'):
            self.generate()

    def test_duplicate_declarations_are_rejected(self) -> None:
        self.config['declarations'].append(self.declaration.copy())
        with self.assertRaisesRegex(ValueError, 'duplicate license declaration'):
            self.generate()

    def test_terms_source_must_identify_the_declared_spdx_license(self) -> None:
        for source in (
            self.config['terms']['MIT']['source'].replace('/MIT.txt', '/BSD-3-Clause.txt'),
            self.config['terms']['MIT']['source'].replace('/spdx/', '/unrelated/'),
        ):
            with self.subTest(source=source):
                configured = self.config['terms']['MIT']['source']
                self.config['terms']['MIT']['source'] = source
                with self.assertRaisesRegex(ValueError, 'SPDX terms source does not match'):
                    self.generate()
                self.config['terms']['MIT']['source'] = configured

    def test_symlinks_cannot_replace_package_or_standard_terms_sources(self) -> None:
        outside = self.root / 'outside.toml'
        outside.write_text(self.original)
        path = self.package_root / 'Cargo.toml.orig'
        path.unlink()
        path.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'declaration escapes package'):
            self.generate()
        path.unlink()
        path.write_text(self.original)
        with tempfile.TemporaryDirectory() as directory:
            outside = Path(directory) / 'MIT.txt'
            outside.write_text(self.template)
            terms = self.root / 'MIT.txt'
            terms.unlink()
            terms.symlink_to(outside)
            with self.assertRaisesRegex(ValueError, 'source escapes manifest directory'):
                self.generate()


if __name__ == '__main__':
    unittest.main()
