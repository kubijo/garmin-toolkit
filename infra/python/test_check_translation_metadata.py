"""Tests for translation source-metadata validation."""

import unittest

from check_translation_metadata import metadata_errors


class MetadataErrorsTests(unittest.TestCase):
    """Validate actionable source and description diagnostics."""

    def test_matching_metadata_has_no_errors(self) -> None:
        source = {'key': {'message': 'Hello', 'description': 'Greeting'}}
        translations = {
            'key': {
                'source': 'Hello',
                'description': 'Greeting',
                'translation': 'Ahoj',
            }
        }

        self.assertEqual(metadata_errors(source, translations), [])

    def test_reports_source_and_description_mismatches(self) -> None:
        source = {'key': {'message': 'Hello', 'description': 'Greeting'}}
        translations = {
            'key': {
                'source': 'Goodbye',
                'description': 'Farewell',
                'translation': 'Ahoj',
            }
        }

        self.assertEqual(
            metadata_errors(source, translations),
            [
                "key: recorded source 'Goodbye'; expected 'Hello'",
                "key: recorded description 'Farewell'; expected 'Greeting'",
            ],
        )

    def test_leaves_key_completeness_to_formatjs(self) -> None:
        source = {'missing': {'message': 'Missing'}}
        translations = {'extra': {'source': 'Extra', 'translation': 'Navíc'}}

        self.assertEqual(metadata_errors(source, translations), [])

    def test_reports_missing_or_blank_translations(self) -> None:
        source = {
            'blank': {'message': 'Blank'},
            'missing': {'message': 'Missing'},
        }
        translations = {
            'blank': {'source': 'Blank', 'translation': '  '},
            'missing': {'source': 'Missing'},
        }

        self.assertEqual(
            metadata_errors(source, translations),
            [
                'blank: translation must be a non-blank string',
                'missing: translation must be a non-blank string',
            ],
        )


if __name__ == '__main__':
    unittest.main()
