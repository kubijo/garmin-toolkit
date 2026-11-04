"""Run upstream test logic and compare the same descriptors with Rust.

This file depends only on this project's locked environment, Python's standard library, and the
crate's conformance example. It has no application/repository tooling imports.
"""

import ast
import hashlib
import importlib.metadata
import json
import os
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from typing import cast

from frictionless import Package

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / 'tests' / 'upstream'


def manifest() -> dict[str, object]:
    return cast(dict[str, object], json.loads((UPSTREAM / 'manifest.json').read_text()))


def strings(value: object) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ValueError('expected manifest string array')
    return cast(list[str], value)


def inline_descriptor(name: str) -> dict[str, object]:
    tree = ast.parse((UPSTREAM / 'test_validate.py').read_text())
    function = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == name)
    for node in ast.walk(function):
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id in ('source', 'descriptor') for target in node.targets
        ):
            return cast(dict[str, object], ast.literal_eval(node.value))
        if (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == 'validate_descriptor'
        ):
            return cast(dict[str, object], ast.literal_eval(node.args[0]))
    raise ValueError(f'no literal descriptor in upstream test {name}')


class UpstreamTests(unittest.TestCase):
    def test_upstream_test_logic_is_unchanged(self) -> None:
        source = cast(dict[str, str], manifest()['python_test'])
        reference = ast.parse((UPSTREAM / source['reference']).read_text())
        executable = ast.parse((UPSTREAM / source['path']).read_text())
        self.assertEqual(
            ast.dump(executable, include_attributes=False),
            ast.dump(reference, include_attributes=False),
        )

    def test_compatibility_tool_versions(self) -> None:
        project = tomllib.loads((ROOT / 'compat/pyproject.toml').read_text())
        for requirement in project['project']['dependencies']:
            name, version = requirement.split('==')
            self.assertEqual(importlib.metadata.version(name), version)

    def test_upstream_bytes_are_unchanged(self) -> None:
        hashes = manifest()['files']
        self.assertIsInstance(hashes, dict)
        for path, expected in cast(dict[str, str], hashes).items():
            with self.subTest(path=path):
                self.assertEqual(hashlib.sha256((UPSTREAM / path).read_bytes()).hexdigest(), expected)

    def test_original_python_tests(self) -> None:
        tests = strings(manifest()['tests'])
        subprocess.run(
            [
                sys.executable,
                '-m',
                'pytest',
                '-q',
                '-p',
                'no:cacheprovider',
                '-o',
                'markers=ci: upstream CI marker\nvcr: upstream HTTP recording marker',
                *[f'test_validate.py::{name}' for name in tests],
            ],
            cwd=UPSTREAM,
            check=True,
        )

    def compare(self, descriptor: Path, *, cwd: Path | None = None) -> None:
        reader = os.environ.get('DATAPACKAGE_READER')
        if reader is None:
            self.fail('set DATAPACKAGE_READER to the compiled conformance example')
        reader = str(Path(reader).resolve())
        source = descriptor if cwd is None else cwd / descriptor
        metadata = cast(dict[str, object], json.loads(source.read_text()))
        official = Package.validate_descriptor(metadata)
        if official.valid:
            official = Package(str(source)).validate()
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'roundtrip.zip'
            result = subprocess.run(
                [reader, str(descriptor), '--roundtrip', str(output)],
                cwd=cwd,
                check=True,
                capture_output=True,
                text=True,
            )
            observed = cast(dict[str, object], json.loads(result.stdout))
            self.assertEqual(observed['valid'], official.valid, result.stdout)
            if observed.get('error') is None:
                issues = cast(list[dict[str, object]], observed['issues'])
                self.assertEqual(
                    sorted(str(issue['code']) for issue in issues),
                    sorted(str(row[0]) for row in official.flatten(['type'])),
                )
            if official.valid:
                self.assertTrue(Package(str(output)).validate().valid)

    def test_real_upstream_fixtures(self) -> None:
        for case in strings(manifest()['cases']):
            with self.subTest(case=case):
                self.compare(UPSTREAM / case)

    def test_literals_from_original_python_tests(self) -> None:
        for case in strings(manifest()['inline_cases']):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'datapackage.json'
                path.write_text(json.dumps(inline_descriptor(case)))
                self.compare(path)

    def test_name_version_gates(self) -> None:
        for version in (None, '1.0', '2.0'):
            for resource_version in (None, '1.0', '2.0'):
                for package_name, resource_name in (
                    ('valid-name_1.0/path', 'table'),
                    ('My Package', 'table'),
                    ('package', 'UPPER CASE'),
                ):
                    with (
                        self.subTest(
                            version=version,
                            resource_version=resource_version,
                            package_name=package_name,
                            resource_name=resource_name,
                        ),
                        tempfile.TemporaryDirectory() as directory,
                    ):
                        resource: dict[str, object] = {'name': resource_name, 'data': [['a'], ['value']]}
                        if resource_version is not None:
                            resource['$schema'] = (
                                f'https://datapackage.org/profiles/{resource_version}/dataresource.json'
                            )
                        descriptor: dict[str, object] = {'name': package_name, 'resources': [resource]}
                        if version is not None:
                            descriptor['$schema'] = f'https://datapackage.org/profiles/{version}/datapackage.json'
                        path = Path(directory) / 'datapackage.json'
                        path.write_text(json.dumps(descriptor))
                        self.compare(path)

    def test_blank_csv_records(self) -> None:
        for row in (',\n', '"",""\n', '\n', '\r\n', ',value\n'):
            with self.subTest(row=row), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'table.csv').write_bytes(f'a,b\n{row}'.encode())
                descriptor = {'resources': [{'name': 'table', 'path': 'table.csv'}]}
                path = root / 'datapackage.json'
                path.write_text(json.dumps(descriptor))
                self.compare(path)

    def test_bare_descriptor_filename(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'table.csv').write_text('a\nvalue\n')
            (root / 'datapackage.json').write_text(
                json.dumps(
                    {
                        'resources': [{'name': 'table', 'path': 'table.csv'}],
                    }
                )
            )
            self.compare(Path('datapackage.json'), cwd=root)


if __name__ == '__main__':
    unittest.main()
