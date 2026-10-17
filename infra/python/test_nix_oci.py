"""Check native OCI output against our runtime settings and real registry transport."""

import gzip
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path

from fixtures.registry import registry
from haos_platform import inspect_archive, unpack_archive

SCRATCH = Path(__file__).resolve().parents[2] / '.tmp'


class NativeOciTests(unittest.TestCase):
    def setUp(self) -> None:
        SCRATCH.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='native-oci-tests.', dir=SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.custom = self.root / 'root'
        self.custom.mkdir()
        (self.custom / 'data').mkdir()
        (self.custom / 'tmp').mkdir(mode=0o1777)
        (self.custom / 'tmp').chmod(0o1777)
        (self.custom / 'probe').write_bytes(b'Native OCI fixture\n')
        os.link(self.custom / 'probe', self.custom / 'probe-copy')

    def build(self, output: Path, *, archive: bool = False) -> None:
        with output.open('xb') if archive else nullcontext(subprocess.PIPE) as destination:
            subprocess.run(
                [
                    'nix-oci',
                    'build',
                    *(['--archive'] if archive else ['--output', str(output)]),
                    '--custom-layer',
                    str(self.custom),
                    '--entrypoint',
                    '/bin/probe',
                    '--working-dir',
                    '/data',
                    '--volumes',
                    '/data',
                    '--exposed-ports',
                    '8099/tcp',
                    '--label',
                    'io.hass.type=app',
                    '--label',
                    'io.hass.arch=amd64',
                    '--label',
                    'io.hass.version=0.1.0',
                    '--annotation',
                    'org.opencontainers.image.source=https://github.com/kubijo/garmin-toolkit',
                    '--ref',
                    'ghcr.io/kubijo/garmin-toolkit-hass:0.1.0',
                ],
                input='',
                stdout=destination,
                stderr=subprocess.PIPE,
                check=True,
                text=True,
                timeout=30,
            )

    @staticmethod
    def blob(layout: Path, descriptor: dict[str, object]) -> Path:
        return layout / 'blobs/sha256' / str(descriptor['digest']).removeprefix('sha256:')

    def test_runtime_config_hardlinks_and_deterministic_manifests(self) -> None:
        first, second = self.root / 'first', self.root / 'second'
        self.build(first)
        self.build(second)
        self.assertEqual(
            {path.relative_to(first): path.read_bytes() for path in first.rglob('*') if path.is_file()},
            {path.relative_to(second): path.read_bytes() for path in second.rglob('*') if path.is_file()},
        )
        index = json.loads((first / 'index.json').read_bytes())
        manifest = json.loads(self.blob(first, index['manifests'][0]).read_bytes())
        configuration = json.loads(self.blob(first, manifest['config']).read_bytes())
        self.assertEqual(configuration['config']['Volumes'], {'/data': {}})
        self.assertEqual(configuration['config']['ExposedPorts'], {'8099/tcp': {}})
        self.assertEqual(configuration['config']['WorkingDir'], '/data')
        archive = self.root / 'image.oci.tar'
        self.build(archive, archive=True)
        inspected = inspect_archive(archive)
        self.assertEqual(inspected.manifest_digest, index['manifests'][0]['digest'])
        unpacked = self.root / 'unpacked'
        unpack_archive(archive, unpacked)
        self.assertEqual(
            {path.relative_to(first): path.read_bytes() for path in first.rglob('*') if path.is_file()},
            {path.relative_to(unpacked): path.read_bytes() for path in unpacked.rglob('*') if path.is_file()},
        )
        with (
            gzip.open(self.blob(first, manifest['layers'][0]), 'rb') as decoded,
            tarfile.open(fileobj=decoded) as layer,
        ):
            entries = {entry.name: entry for entry in layer}
            self.assertEqual(entries['tmp'].mode, 0o1777)
            self.assertTrue(entries['probe-copy'].islnk())
            self.assertEqual(entries['probe-copy'].linkname, 'probe')

    def test_registry_preserves_the_native_manifest_without_docker_conversion(self) -> None:
        layout = self.root / 'image'
        self.build(layout)
        descriptor = json.loads((layout / 'index.json').read_bytes())['manifests'][0]
        with registry(self.root) as repository:
            subprocess.run(
                [
                    'skopeo',
                    '--insecure-policy',
                    '--tmpdir',
                    str(self.root),
                    'copy',
                    '--all',
                    '--preserve-digests',
                    '--image-parallel-copies',
                    '1',
                    '--dest-no-creds',
                    '--dest-tls-verify=false',
                    'oci:' + str(layout),
                    'docker://' + repository + ':native',
                ],
                check=True,
                capture_output=True,
                text=True,
                timeout=60,
            )
            raw = subprocess.check_output(
                [
                    'skopeo',
                    'inspect',
                    '--raw',
                    '--no-creds',
                    '--tls-verify=false',
                    'docker://' + repository + '@' + descriptor['digest'],
                ],
                timeout=30,
            )
        self.assertEqual(raw, self.blob(layout, descriptor).read_bytes())


if __name__ == '__main__':
    unittest.main()
