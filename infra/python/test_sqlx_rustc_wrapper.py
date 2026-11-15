"""Keep dependency query metadata separate from workspace SQLx preparation."""

import json
import os
import subprocess
import sys
import unittest
from pathlib import Path
from typing import cast

WRAPPER = Path(__file__).resolve().parents[1] / 'just/sqlx-rustc-wrapper.sh'
KEYS = ('SQLX_OFFLINE', 'SQLX_OFFLINE_DIR', 'SQLX_TMP', 'DATABASE_URL')


class SqlxRustcWrapperTests(unittest.TestCase):
    def run_wrapper(self, primary: bool, chained: bool = False) -> dict[str, str]:
        environment = dict(os.environ)
        environment.pop('CARGO_PRIMARY_PACKAGE', None)
        environment.pop('GARMIN_SQLX_RUSTC_WRAPPER', None)
        environment.update(
            dict(zip(KEYS, ('false', '/query-cache', '/query-temp', 'sqlite://query.sqlite3'), strict=True))
        )
        if primary:
            environment['CARGO_PRIMARY_PACKAGE'] = '1'
        if chained:
            # env is a transparent outer wrapper, like a configured compiler cache.
            environment['GARMIN_SQLX_RUSTC_WRAPPER'] = 'env'
        result = subprocess.run(
            ['bash', str(WRAPPER), sys.executable, '-c', 'import json, os; print(json.dumps(dict(os.environ)))'],
            env=environment,
            check=True,
            capture_output=True,
            text=True,
        )
        return cast(dict[str, str], json.loads(result.stdout))

    def test_workspace_queries_keep_online_database_and_output(self) -> None:
        for chained in (False, True):
            with self.subTest(chained=chained):
                environment = self.run_wrapper(primary=True, chained=chained)
                self.assertEqual(
                    [environment[key] for key in KEYS],
                    ['false', '/query-cache', '/query-temp', 'sqlite://query.sqlite3'],
                )

    def test_dependencies_use_their_own_offline_metadata_without_emitting_queries(self) -> None:
        for chained in (False, True):
            with self.subTest(chained=chained):
                environment = self.run_wrapper(primary=False, chained=chained)
                self.assertEqual(environment['SQLX_OFFLINE'], 'true')
                for key in KEYS[1:]:
                    self.assertNotIn(key, environment)


if __name__ == '__main__':
    unittest.main()
