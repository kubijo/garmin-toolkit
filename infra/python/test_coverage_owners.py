"""Check ownership and denominator handling before using reports to choose coverage policy."""

import tempfile
import unittest
from pathlib import Path

from coverage_owners import Counts, Owner, Policy, enforce, parse_lcov, report_data, summarize, workspace_owners


def record(path: str, covered: int, total: int) -> str:
    return f'SF:{path}\nLF:{total}\nLH:{covered}\nend_of_record\n'


class CoverageOwnersTests(unittest.TestCase):
    def test_global_pass_does_not_hide_an_uncovered_owner(self) -> None:
        root = Path('/project')
        files = parse_lcov(record('large/src/lib.rs', 100, 100) + record('small/src/lib.rs', 0, 10), root)
        report = summarize(files, [Owner('large', root / 'large'), Owner('small', root / 'small')], root)
        total = sum((row.lines for row in report.owners), Counts())
        self.assertGreater(total.percent or 0, 65)
        self.assertEqual(report.owners[0].owner.name, 'small')
        self.assertEqual(report.owners[0].lines.percent, 0)
        with self.assertRaisesRegex(ValueError, 'small:'):
            enforce(report, Policy({'large': 65, 'small': 65}, {}))

    def test_exact_boundary_passes_but_rounding_cannot_pass(self) -> None:
        root = Path('/project')
        for covered in [6499, 6500]:
            report = summarize(parse_lcov(record('a/lib.rs', covered, 10000), root), [Owner('a', root / 'a')], root)
            if covered == 6500:
                enforce(report, Policy({'a': 65}, {}))
            else:
                with self.assertRaisesRegex(ValueError, 'requires 65%'):
                    enforce(report, Policy({'a': 65}, {}))

    def test_policy_must_account_for_every_owner_and_actual_measurement(self) -> None:
        root = Path('/project')
        report = summarize(parse_lcov(record('a/lib.rs', 1, 1), root), [Owner('a', root / 'a')], root)
        for policy in [Policy({}, {}), Policy({'a': 100, 'removed': 50}, {}), Policy({}, {'a': 'WASM'})]:
            with self.subTest(policy=policy), self.assertRaises(ValueError):
                enforce(report, policy)
        empty = summarize(parse_lcov(record('/external.rs', 1, 1), root), [Owner('a', root / 'a')], root)
        enforce(empty, Policy({}, {'a': 'WASM'}))
        with self.assertRaises(ValueError):
            enforce(empty, Policy({'a': 0}, {}))

    def test_nested_cargo_owner_is_not_counted_in_its_parent(self) -> None:
        root = Path('/project')
        files = parse_lcov(record('cli/src/main.rs', 5, 10) + record('cli/tui/src/lib.rs', 20, 20), root)
        report = summarize(files, [Owner('cli', root / 'cli'), Owner('tui', root / 'cli/tui')], root)
        self.assertEqual([(row.owner.name, row.lines.total) for row in report.owners], [('cli', 10), ('tui', 20)])

    def test_unmeasured_owners_and_unassigned_files_remain_visible(self) -> None:
        root = Path('/project')
        files = parse_lcov(record('extra/file.rs', 1, 2) + record('/external/file.rs', 3, 4), root)
        report = summarize(files, [Owner('ui', root / 'ui')], root)
        data = report_data(report, root)
        self.assertIsNone(report.owners[0].lines.percent)
        self.assertEqual(len(report.unassigned), 1)
        self.assertEqual(len(report.external), 1)
        self.assertIn('unassigned', data)

    def test_malformed_or_duplicate_records_cannot_inflate_coverage(self) -> None:
        valid = record('a.rs', 1, 2)
        for invalid in (
            '',
            valid + valid,
            record('a.rs', 3, 2),
            'SF:a.rs\nLF:2\nend_of_record\n',
            'SF:a.rs\nLF:2\nLH:1\n',
            'SF:a.rs\nLF:2\nLF:2\nLH:1\nend_of_record\n',
        ):
            with self.subTest(source=invalid), self.assertRaises(ValueError):
                parse_lcov(invalid, Path('/project'))

    def test_workspace_members_define_owners_and_exclusions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.toml').write_text('[workspace]\nmembers=["crates/*"]\nexclude=["crates/excluded"]\n')
            for name in ['first', 'second', 'excluded']:
                crate = root / 'crates' / name
                crate.mkdir(parents=True)
                (crate / 'Cargo.toml').write_text(f'[package]\nname="{name}"\n')
            self.assertEqual({owner.name for owner in workspace_owners(root)}, {'first', 'second'})


if __name__ == '__main__':
    unittest.main()
