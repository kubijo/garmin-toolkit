"""Run bounded GitHub/GHCR recovery drills on uniquely owned synthetic fixtures."""

import errno
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Literal

import tyro
from rich.text import Text

from cli_output import TerminalOutput
from hass_ci import progress, require_hosted_runner
from hass_publish import IMAGE, REMOTE, REPOSITORY, check_image_version, command, git
from hass_recovery_git import StoreDrill
from hass_registry import CONFIG, INDEX, MANIFEST, write_blob
from hass_release_contract import check_directory
from hass_store import Store, oid, write_journal
from json_data import Object, file_sha256, is_array, json_bytes, object_rows, object_value, read_json, strings, text

ROOT = Path('.tmp/recovery')
AUTH = Path('.tmp/recovery-auth/config.json')
OUTPUT = TerminalOutput()


@dataclass(frozen=True)
class Target:
    run: str
    attempt: str
    commit: str

    def __post_init__(self) -> None:
        if any(re.fullmatch(r'[1-9][0-9]{0,19}', value) is None for value in (self.run, self.attempt)):
            raise ValueError('Recovery requires the actual GitHub run ID and attempt')
        oid(self.commit)

    @property
    def prefix(self) -> str:
        return 'hass-recovery-' + self.run + '-' + self.attempt

    @property
    def package(self) -> str:
        return 'garmin-toolkit-recovery-' + self.run + '-' + self.attempt

    @property
    def image(self) -> str:
        return 'ghcr.io/kubijo/' + self.package

    @property
    def refs(self) -> tuple[str, ...]:
        return (
            'refs/heads/' + self.prefix + '-apps',
            'refs/heads/' + self.prefix + '-state',
            'refs/tags/' + self.prefix,
        )

    def identity(self) -> Object:
        return {'repository': REPOSITORY, 'run': self.run, 'attempt': self.attempt, 'commit': self.commit}


def api(resource: str) -> Object | None:
    try:
        return object_value(json.loads(command('gh', 'api', resource)))
    except subprocess.CalledProcessError as error:
        if '(HTTP 404)' not in (error.stderr or ''):
            raise
        return None


def fixture_image(directory: Path, marker: str) -> str:
    """Write a tiny, layer-free OCI fixture; it is not a runnable app or a release candidate."""
    (directory / 'blobs/sha256').mkdir(parents=True)
    config: Object = {
        'mediaType': CONFIG,
        'architecture': 'amd64',
        'os': 'linux',
        'rootfs': {'type': 'layers', 'diff_ids': []},
        'config': {'Labels': {'org.opencontainers.image.source': REMOTE, 'recovery.fixture': marker}},
    }
    manifest = write_blob(
        directory,
        {'mediaType': MANIFEST, 'schemaVersion': 2, 'config': write_blob(directory, config), 'layers': []},
    )
    index = write_blob(
        directory,
        {
            'mediaType': INDEX,
            'schemaVersion': 2,
            'manifests': [{**manifest, 'platform': {'os': 'linux', 'architecture': 'amd64'}}],
        },
    )
    (directory / 'index.json').write_bytes(json_bytes({'schemaVersion': 2, 'manifests': [index]}))
    (directory / 'oci-layout').write_bytes(json_bytes({'imageLayoutVersion': '1.0.0'}))
    return text(index['digest'])


@dataclass(frozen=True)
class Drill:
    target: Target
    root: Path

    def head(self, reference: str) -> str:
        if reference not in self.target.refs:
            raise ValueError('Recovery cannot inspect a fixture ref outside its owned namespace')
        result = git('ls-remote', '--refs', REMOTE, reference)
        if not result:
            return ''
        commit, _, ref = result.partition('\t')
        if ref != reference:
            raise ValueError('Unexpected recovery fixture ref response')
        return oid(commit)

    def execute(self, *arguments: str, cwd: Path | None = None) -> str:
        if 'push' in arguments:
            position = arguments.index('push')
            options = arguments[position + 1 :]
            if REMOTE not in options or options.count(REMOTE) != 1:
                raise ValueError('Recovery pushes require the canonical GitHub host')
            destination = options.index(REMOTE)
            references = options[destination + 1 :]
            if not references:
                raise ValueError('Recovery push has no explicit refs')
            leases = set(options[:destination])
            for value in leases:
                if value == '--atomic':
                    continue
                lease = value.removeprefix('--force-with-lease=')
                reference, separator, expected = lease.partition(':')
                if not value.startswith('--force-with-lease=') or not separator or reference not in self.target.refs:
                    raise ValueError('Recovery refused an unrestricted push option')
                oid(expected, empty=True)
            journal = object_value(read_json(self.root / 'refs.json'))
            for spec in references:
                commit, separator, reference = spec.partition(':')
                if not separator or reference not in self.target.refs:
                    raise ValueError('Recovery refused a push outside its owned fixture refs')
                oid(commit, empty=True)
                if not any(value.startswith('--force-with-lease=' + reference + ':') for value in leases):
                    raise ValueError('Recovery requires an exact lease for every written ref')
                known = strings(journal.get(reference, []))
                if commit and commit not in known:
                    journal[reference] = [*known, commit]
            write_journal(self.root, journal, replace=True, name='refs.json')
        return git(*arguments, cwd=cwd)

    def store(self) -> Store:
        return Store(
            REMOTE,
            self.execute,
            lambda branch: self.head('refs/heads/' + branch),
            branch=self.target.prefix + '-apps',
            fence=self.target.prefix + '-state',
        )

    def protected_refs(self) -> str:
        return git('ls-remote', '--refs', REMOTE, 'refs/heads/hass-apps', 'refs/heads/hass-publication', 'refs/tags/v*')

    def release(self) -> Object | None:
        # The tag endpoint is for published releases; authenticated enumeration also includes drafts.
        pages: object = json.loads(
            command('gh', 'api', '--paginate', '--slurp', 'repos/' + REPOSITORY + '/releases?per_page=100')
        )
        if not is_array(pages):
            raise ValueError('Invalid paginated recovery release response')
        matches = [
            release for page in pages for release in object_rows(page) if release.get('tag_name') == self.target.prefix
        ]
        if len(matches) > 1:
            raise ValueError('Multiple releases identify the same recovery fixture')
        return matches[0] if matches else None

    def digest(self, version: str) -> str:
        raw = subprocess.run(
            ['skopeo', 'inspect', '--raw', '--authfile', str(AUTH), 'docker://' + self.target.image + ':' + version],
            check=True,
            capture_output=True,
            timeout=60,
        ).stdout
        return 'sha256:' + hashlib.sha256(raw).hexdigest()

    def copy(self, directory: Path, version: str) -> None:
        command(
            'skopeo',
            '--command-timeout',
            '2m',
            'copy',
            '--all',
            '--preserve-digests',
            '--image-parallel-copies',
            '1',
            '--authfile',
            str(AUTH),
            'oci:' + str(directory),
            'docker://' + self.target.image + ':' + version,
        )

    def download(self, name: str, expected: set[str]) -> Path:
        directory = self.root / name
        directory.mkdir()
        command('gh', 'release', 'download', self.target.prefix, '--repo', REPOSITORY, '--dir', str(directory))
        check_directory(directory, expected)
        return directory

    def run(self) -> None:
        progress('Checking fresh recovery targets...')
        if self.root.exists() or self.root.is_symlink():
            raise ValueError('Recovery evidence already exists; retain its original journals')
        if self.target.image == IMAGE or any(self.head(ref) for ref in self.target.refs):
            raise ValueError('Recovery requires unused fixture refs and a separate package')
        if api('users/kubijo/packages/container/' + self.target.package) is not None:
            raise ValueError('Recovery fixture package already exists')
        if self.release() is not None:
            raise ValueError('Recovery fixture release already exists')
        self.root.mkdir(parents=True)
        write_journal(self.root, self.target.identity(), name='ownership.json')
        write_journal(self.root, {}, name='refs.json')
        protected = self.protected_refs()
        write_journal(self.root, {'refs': protected}, name='protected.json')
        progress('Checking GHCR version conflicts and a greater replacement...')
        baseline = fixture_image(self.root / 'image-baseline', 'baseline')
        candidate = fixture_image(self.root / 'image-candidate', 'candidate')
        (self.root / 'registry.json').write_bytes(
            json_bytes({'image': self.target.image, 'baseline': baseline, 'candidate': candidate})
        )
        write_journal(self.root, self.target.identity(), name='package-owned.json')
        self.copy(self.root / 'image-baseline', '0.0.1')
        if self.digest('0.0.1') != baseline:
            raise RuntimeError('GHCR changed the fixture index')
        tags = object_value(
            json.loads(command('skopeo', 'list-tags', '--authfile', str(AUTH), 'docker://' + self.target.image))
        )
        try:
            check_image_version('0.0.1', candidate, set(strings(tags['Tags'])), self.digest)
        except ValueError as error:
            if 'refusing to replace' not in str(error):
                raise
        else:
            raise RuntimeError('A conflicting GHCR version was accepted')
        check_image_version('0.0.1', baseline, set(strings(tags['Tags'])), self.digest)
        if self.digest('0.0.1') != baseline:
            raise RuntimeError('Conflict rejection changed the existing GHCR version')
        check_image_version('0.0.2', candidate, set(strings(tags['Tags'])), self.digest)
        self.copy(self.root / 'image-candidate', '0.0.2')
        if self.digest('0.0.2') != candidate:
            raise RuntimeError('GHCR changed the replacement fixture')

        progress('Checking incomplete GitHub draft downloads...')
        tag = self.target.refs[2]
        self.execute('push', '--force-with-lease=' + tag + ':', REMOTE, self.target.commit + ':' + tag)
        command(
            'gh',
            'release',
            'create',
            self.target.prefix,
            '--repo',
            REPOSITORY,
            '--verify-tag',
            '--draft',
            '--title',
            'Synthetic publication recovery fixture',
            '--notes',
            'Disposable transport fixture. Not an app release; no installation or provenance qualification is claimed.',
        )
        payload = self.root / 'fixture.json'
        payload.write_bytes(json_bytes({'synthetic': True, 'index_digest': candidate}))
        receipt = self.root / 'receipt.json'
        receipt.write_bytes(json_bytes({'name': payload.name, 'sha256': file_sha256(payload)}))
        expected = {payload.name, receipt.name}
        command('gh', 'release', 'upload', self.target.prefix, '--repo', REPOSITORY, str(payload))
        try:
            self.download('incomplete', expected)
        except ValueError as error:
            if 'inventory differs' not in str(error):
                raise
        else:
            raise RuntimeError('An incomplete GitHub draft was accepted')
        if any(self.head(ref) for ref in self.target.refs[:2]):
            raise RuntimeError('Incomplete draft handling advertised fixture metadata')
        command('gh', 'release', 'upload', self.target.prefix, '--repo', REPOSITORY, str(receipt))
        downloaded = self.download('complete', expected)
        for original in (payload, receipt):
            if file_sha256(downloaded / original.name) != file_sha256(original):
                raise RuntimeError('GitHub draft download changed fixture bytes')
        progress('Exercising Git recovery against owned fixture refs...')
        store = StoreDrill(self.store(), self.root / 'store').run()
        progress('Checking retained artifacts and protected public refs...')
        retained = self.download('retained', expected)
        for original in (payload, receipt):
            if file_sha256(retained / original.name) != file_sha256(original):
                raise RuntimeError('Recovery replaced a fixture release asset')
        if self.digest('0.0.1') != baseline or self.digest('0.0.2') != candidate:
            raise RuntimeError('Store recovery replaced a fixture image')
        if self.protected_refs() != protected:
            raise RuntimeError('Public release/store refs changed during the drill')
        (self.root / 'result.json').write_bytes(
            json_bytes(
                {
                    'passed': True,
                    'scope': 'synthetic transport fixtures; no signed release or Supervisor acceptance',
                    'target': self.target.identity(),
                    'version_conflict_rejected': True,
                    'incomplete_draft_rejected': True,
                    'release_assets_preserved': True,
                    'registry_versions_preserved': True,
                    'public_refs_unchanged': True,
                    'store': store,
                }
            )
        )

    def cleanup(self) -> None:
        progress('Cleaning owned recovery fixtures...')
        if not self.root.exists():
            return
        if object_value(read_json(self.root / 'ownership.json')) != self.target.identity():
            raise ValueError('Recovery cleanup does not own this evidence')
        release = self.release()
        if release is not None:
            if (
                release.get('draft') is not True
                or release.get('tag_name') != self.target.prefix
                or self.head(self.target.refs[2]) != self.target.commit
            ):
                raise ValueError('Refusing to remove a fixture that became a public release')
            assets = object_rows(release['assets'])
            if any(asset.get('name') not in ('fixture.json', 'receipt.json') for asset in assets):
                raise ValueError('Refusing to remove unrelated draft assets')
            if assets:
                with tempfile.TemporaryDirectory(prefix='.cleanup.', dir=self.root) as temporary:
                    downloaded = self.download(
                        Path(temporary).name + '/assets', {text(asset['name']) for asset in assets}
                    )
                    for asset in assets:
                        name = text(asset['name'])
                        if file_sha256(downloaded / name) != file_sha256(self.root / name):
                            raise ValueError('Refusing to remove changed draft asset bytes')
            command('gh', 'api', '--method', 'DELETE', 'repos/' + REPOSITORY + '/releases/' + str(release['id']))
        journal = object_value(read_json(self.root / 'refs.json'))
        for reference in self.target.refs:
            current = self.head(reference)
            if current:
                if current not in strings(journal.get(reference, [])):
                    raise ValueError('Fixture ref changed to an unowned commit; retaining it')
                self.execute('push', '--force-with-lease=' + reference + ':' + current, REMOTE, ':' + reference)
        package_path = 'users/kubijo/packages/container/' + self.target.package
        if (self.root / 'package-owned.json').is_file() and (package := api(package_path)) is not None:
            repository = object_value(package.get('repository'))
            if package.get('name') != self.target.package or repository.get('full_name') != REPOSITORY:
                raise ValueError('Refusing to delete an unrelated GHCR package')
            known = {
                'sha256:' + path.name
                for name in ('image-baseline', 'image-candidate')
                for path in (self.root / name / 'blobs/sha256').iterdir()
                if object_value(read_json(path)).get('mediaType') in (INDEX, MANIFEST)
            }
            versions = object_rows(json.loads(command('gh', 'api', package_path + '/versions?per_page=100')))
            if len(versions) >= 100 or any(version.get('name') not in known for version in versions):
                raise ValueError('Refusing to remove unowned GHCR image versions')
            command('gh', 'api', '--method', 'DELETE', package_path)
        if (
            any(self.head(ref) for ref in self.target.refs)
            or self.release() is not None
            or api(package_path) is not None
        ):
            raise RuntimeError('Recovery fixture cleanup did not complete')
        if self.protected_refs() != object_value(read_json(self.root / 'protected.json'))['refs']:
            raise RuntimeError('Public release/store refs changed during cleanup')
        write_journal(self.root, {'passed': True, 'target': self.target.identity()}, replace=True, name='cleanup.json')


@dataclass(frozen=True)
class Options:
    command: Annotated[Literal['run', 'cleanup'], tyro.conf.Positional]


def main(options: Options) -> int:
    try:
        require_hosted_runner()
        if os.environ.get('GITHUB_REPOSITORY') != REPOSITORY:
            raise ValueError('Recovery is restricted to the canonical repository workflow')
        target = Target(
            os.environ.get('GITHUB_RUN_ID', ''),
            os.environ.get('GITHUB_RUN_ATTEMPT', ''),
            os.environ.get('GITHUB_SHA', ''),
        )
        if git('rev-parse', 'HEAD') != target.commit:
            raise ValueError('Recovery requires the exact workflow checkout commit')
        drill = Drill(target, ROOT)
        if options.command == 'cleanup':
            AUTH.unlink(missing_ok=True)
            drill.cleanup()
        else:
            if AUTH.parent.exists() or AUTH.parent.is_symlink():
                raise ValueError('Recovery requires fresh isolated registry credentials')
            AUTH.parent.mkdir(mode=0o700, parents=True)
            try:
                subprocess.run(
                    [
                        'skopeo',
                        'login',
                        '--authfile',
                        str(AUTH),
                        '--username',
                        os.environ['GITHUB_ACTOR'],
                        '--password-stdin',
                        'ghcr.io',
                    ],
                    input=os.environ['GH_TOKEN'],
                    text=True,
                    check=True,
                    capture_output=True,
                    timeout=60,
                )
                AUTH.chmod(0o600)
                drill.run()
            finally:
                AUTH.unlink(missing_ok=True)
    except subprocess.CalledProcessError as error:
        details = error.stderr or error.stdout or str(error)
        OUTPUT.ansi(
            (details.decode(errors='replace') if isinstance(details, bytes) else details)[-4096:],
            title='Recovery drill failed',
            error=True,
        )
        return error.returncode if error.returncode > 0 else 128 - error.returncode
    except subprocess.TimeoutExpired:
        OUTPUT.panel(Text('A recovery operation timed out.'), title='Recovery drill failed', error=True)
        return 124
    except (ValueError, OSError, KeyError) as error:
        OUTPUT.panel(Text(str(error)), title='Recovery drill rejected', error=True)
        if isinstance(error, OSError) and error.errno in (errno.ENOENT, errno.EACCES):
            return 127 if error.errno == errno.ENOENT else 126
        return 1
    except KeyboardInterrupt:
        return 130
    OUTPUT.panel(Text(str(ROOT)), title='Recovery ' + options.command + ' completed', style='green')
    return 0


if __name__ == '__main__':
    OUTPUT.install_tracebacks()
    sys.exit(main(tyro.cli(Options)))
