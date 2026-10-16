"""Serve generated candidate metadata through a disposable Git HTTP repository."""

import os
import shutil
import subprocess
import threading
from collections.abc import Generator
from contextlib import contextmanager
from email.parser import BytesHeaderParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

import yaml

from hass_release_contract import check_shape
from json_data import object_value, strings


@contextmanager
def serve_repository(directory: Path, runtime: Path, *, image: str, architecture: str, version: str) -> Generator[str]:
    check_shape(directory, 'hass-app-repository.dirspec.toml')
    if directory.is_symlink() or any(path.is_symlink() for path in directory.rglob('*')):
        raise ValueError('Candidate repository must contain regular files and directories')
    working = runtime / 'repository-work'
    shutil.copytree(directory, working)
    config_path = working / 'garmin_toolkit/config.yaml'
    config = object_value(yaml.safe_load(config_path.read_text()))
    if config.get('version') != version or architecture not in strings(config.get('arch')):
        raise ValueError('Candidate repository architecture/version differs from the image')
    config['image'] = image
    config_path.write_text(yaml.safe_dump(config, sort_keys=False))
    environment = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
    environment.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull)

    def git(*arguments: str) -> None:
        subprocess.run(
            [
                'git',
                '-c',
                'user.name=HAOS Candidate',
                '-c',
                'user.email=haos@example.invalid',
                '-c',
                'commit.gpgsign=false',
                '-c',
                f'core.hooksPath={runtime / "no-hooks"}',
                *arguments,
            ],
            env=environment,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )

    git('init', '-q', '--initial-branch=hass-apps', str(working))
    git('-C', str(working), 'add', '.')
    git('-C', str(working), 'commit', '-qm', 'Disposable generated candidate')
    published = runtime / 'repository-http'
    published.mkdir()
    bare = published / 'garmin-toolkit.git'
    git('clone', '--bare', '-q', str(working), str(bare))

    class GitHandler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.respond()

        def do_POST(self) -> None:
            self.respond()

        def respond(self) -> None:
            url = urlsplit(self.path)
            if url.path not in ('/garmin-toolkit.git/info/refs', '/garmin-toolkit.git/git-upload-pack'):
                self.send_error(404)
                return
            size = int(self.headers.get('Content-Length', '0'))
            if not 0 <= size <= 1024 * 1024:
                self.send_error(413)
                return
            response = subprocess.run(
                ['git', 'http-backend'],
                env={
                    **environment,
                    'GIT_PROJECT_ROOT': str(published),
                    'GIT_HTTP_EXPORT_ALL': '1',
                    'REQUEST_METHOD': self.command,
                    'PATH_INFO': url.path,
                    'QUERY_STRING': url.query,
                    'CONTENT_TYPE': self.headers.get('Content-Type', ''),
                    'CONTENT_LENGTH': str(size),
                },
                input=self.rfile.read(size),
                capture_output=True,
                check=True,
                timeout=30,
            ).stdout
            headers, separator, body = response.partition(b'\r\n\r\n')
            if not separator:
                self.send_error(502)
                return
            fields = BytesHeaderParser().parsebytes(headers)
            self.send_response(int(fields.get('Status', '200').split()[0]))
            for name, value in fields.items():
                if name.lower() not in ('status', 'content-length'):
                    self.send_header(name, value)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    with ThreadingHTTPServer(('127.0.0.1', 0), GitHandler) as server:
        worker = threading.Thread(target=server.serve_forever, name='haos-repository')
        worker.start()
        try:
            # QEMU user networking routes this guest address to the host loopback listener.
            yield f'http://10.0.2.2:{server.server_port}/garmin-toolkit.git#hass-apps'
        finally:
            server.shutdown()
            worker.join(timeout=5)
