"""Exercise the patched Trunk with real Cargo, WASM, bindgen, and JavaScript inputs.

Run explicitly through just qa::trunk-cache; ordinary Python tests do not build Rust.
"""

import os
import shutil
import signal
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from build_profile import BASE, ROOT, invoke


class TrunkCacheAcceptance(unittest.TestCase):
    def test_cache_preserves_embedded_and_independent_javascript(self) -> None:
        BASE.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='bindgen-check-', dir=BASE) as temporary:
            root = Path(temporary)
            source = root / 'source'
            shutil.copytree(ROOT / 'infra/python/fixtures/bindgen-cache', source)
            events = root / 'events'
            events.mkdir()
            environment = {
                **os.environ,
                'CARGO_TARGET_DIR': str(root / 'target'),
                'CARGO_BUILD_JOBS': '2',
                'TRUNK_SKIP_VERSION_CHECK': 'true',
                'TRUNK_COLOR': 'never',
            }
            environment.pop('NO_COLOR', None)
            environment.pop('TRUNK_BINDGEN_CACHE_DISABLE', None)

            def build() -> str:
                with (root / 'build.log').open('w+') as log, patch.dict(os.environ, GARMIN_BUILD_TRACE=str(events)):
                    code = invoke(
                        ['trunk', 'build', '--offline', '--locked'],
                        'cache acceptance',
                        group=True,
                        cwd=source,
                        env=environment,
                        stdout=log,
                        stderr=log,
                    )
                    log.seek(0)
                    output = log.read()
                self.assertEqual(code, 0, output)
                return output

            def snippets() -> str:
                return '\n'.join(path.read_text() for path in (source / 'dist/snippets').rglob('*.js'))

            self.assertNotIn('wasm-bindgen cache hit', build())
            self.assertIn('bindgen-cache-before', snippets())
            shutil.rmtree(source / 'dist')
            self.assertIn('wasm-bindgen cache hit', build())
            self.assertIn('bindgen-cache-before', snippets())
            self.assertTrue(list((source / 'dist').glob('*.wasm')))

            # Cargo tracks imported local modules, even when Rust source is unchanged.
            shutil.copyfile(source / 'local-after.js', source / 'local.js')
            self.assertNotIn('wasm-bindgen cache hit', build())
            self.assertIn('bindgen-cache-after', snippets())
            self.assertNotIn('bindgen-cache-before', snippets())

            # A cache hit must still execute the rest of Trunk's asset pipeline.
            shutil.copyfile(source / 'asset-after.js', source / 'asset.js')
            self.assertIn('wasm-bindgen cache hit', build())
            self.assertIn('asset-after', (source / 'dist/asset.js').read_text())


if __name__ == '__main__':

    def interrupted(signum: int, _frame: object) -> None:
        raise SystemExit(128 + signum)

    for termination_signal in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(termination_signal, interrupted)
    unittest.main()
