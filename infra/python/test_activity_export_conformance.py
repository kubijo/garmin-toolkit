"""Independent validator controls based on upstream package validation cases.

Upstream: frictionlessdata/frictionless-py v5.21.0,
frictionless/package/__spec__/test_validate.py (MIT): strict v2 descriptors,
composite primary keys, and hash/byte counts. These are adapted controls, not
a claim that upstream provides a separate language-neutral certification suite.
"""

import json
import tempfile
import unittest
from pathlib import Path

from frictionless import Package

from activity_export_conformance import validate


class ConformanceTests(unittest.TestCase):
    def test_strict_v2_requires_resource_names(self) -> None:
        report = Package.validate_descriptor(
            {
                '$schema': 'https://datapackage.org/profiles/2.0/datapackage.json',
                'resources': [{'path': 'samples.csv'}],
            }
        )
        self.assertFalse(report.valid)

    def test_official_validator_checks_rows_keys_relations_and_integrity(self) -> None:
        cases = (
            ('id,value\n1,2\n', 'id,position\n1,0\n', True),
            ('id,value\n1,wrong\n', 'id,position\n1,0\n', False),
            ('id,value\n1,2\n1,3\n', 'id,position\n1,0\n', False),
            ('id,value\n1,2\n', 'id,position\n2,0\n', False),
            ('id,value\n1,2\n', 'id,position\n1,0\n1,0\n', False),
            ('id,value\n,2\n', 'id,position\n1,0\n', False),
        )
        for parent, child, valid in cases:
            with self.subTest(parent=parent, child=child), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'activities.csv').write_text(parent)
                (root / 'samples.csv').write_text(child)
                descriptor: dict[str, object] = {
                    '$schema': 'https://datapackage.org/profiles/2.0/datapackage.json',
                    'resources': [
                        {
                            'name': 'activities',
                            'type': 'table',
                            'path': 'activities.csv',
                            'schema': {
                                'fields': [
                                    {'name': 'id', 'type': 'integer', 'constraints': {'required': True}},
                                    {'name': 'value', 'type': 'integer'},
                                ],
                                'primaryKey': ['id'],
                            },
                        },
                        {
                            'name': 'samples',
                            'type': 'table',
                            'path': 'samples.csv',
                            'schema': {
                                'fields': [{'name': 'id', 'type': 'integer'}, {'name': 'position', 'type': 'integer'}],
                                'primaryKey': ['id', 'position'],
                                'foreignKeys': [
                                    {'fields': ['id'], 'reference': {'resource': 'activities', 'fields': ['id']}}
                                ],
                            },
                        },
                    ],
                }
                path = root / 'datapackage.json'
                path.write_text(json.dumps(descriptor))
                if valid:
                    validate(path)
                else:
                    with self.assertRaises(ValueError):
                        validate(path)

    def test_invalid_resource_hash_and_byte_count_fail(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'samples.csv').write_text('id\n1\n')
            report = Package(
                {
                    '$schema': 'https://datapackage.org/profiles/2.0/datapackage.json',
                    'resources': [
                        {'name': 'samples', 'type': 'table', 'path': 'samples.csv', 'hash': 'sha256:bad', 'bytes': 1}
                    ],
                },
                basepath=directory,
            ).validate()
            self.assertFalse(report.valid)
            self.assertIn(['hash-count'], report.flatten(['type']))
            self.assertIn(['byte-count'], report.flatten(['type']))


if __name__ == '__main__':
    unittest.main()
