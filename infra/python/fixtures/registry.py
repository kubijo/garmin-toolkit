"""An owned loopback OCI Distribution process, storage tree, and bounded cleanup."""

import os
import socket
import subprocess
import time
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path

import yaml


@contextmanager
def registry(root: Path) -> Generator[str]:
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    configuration = root / 'registry.yaml'
    configuration.write_text(
        yaml.safe_dump(
            {
                'version': 0.1,
                'log': {'level': 'error'},
                'storage': {'filesystem': {'rootdirectory': str(root / 'storage')}},
                'http': {'addr': f'127.0.0.1:{port}', 'secret': 'disposable-registry-fixture'},
            }
        )
    )
    with (root / 'registry.log').open('w+') as log:
        process = subprocess.Popen(
            ['registry', 'serve', str(configuration)],
            stdout=log,
            stderr=subprocess.STDOUT,
            env={**os.environ, 'OTEL_SDK_DISABLED': 'true'},
        )
        try:
            deadline = time.monotonic() + 10
            while True:
                with socket.socket() as connection:
                    if connection.connect_ex(('127.0.0.1', port)) == 0:
                        break
                if process.poll() is not None or time.monotonic() >= deadline:
                    log.seek(0)
                    raise RuntimeError('Disposable registry did not start: ' + log.read())
                time.sleep(0.05)
            yield f'127.0.0.1:{port}/garmin-toolkit-hass'
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            with socket.socket() as connection:
                if connection.connect_ex(('127.0.0.1', port)) == 0:
                    raise RuntimeError('Disposable registry listener survived cleanup')
