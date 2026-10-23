"""Generate the release-only app repository from canonical metadata, docs, and branding."""

import shutil
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path

import yaml

from hass_release_contract import Architecture, check_shape
from json_data import object_value, regular_file, strings, text


def copy_repository(source: Path, destination: Path, *, architectures: tuple[Architecture, ...] | None = None) -> None:
    """Copy generated metadata into writable owned scratch, including Nix store inputs."""
    if source.is_symlink() or any(path.is_symlink() for path in source.rglob('*')):
        raise ValueError('Repository metadata must contain regular files and directories')
    shutil.copytree(source, destination, copy_function=shutil.copyfile)
    for path in (destination, *(path for path in destination.rglob('*') if path.is_dir())):
        path.chmod(0o755)
    if architectures is not None:
        path = destination / 'garmin_toolkit/config.yaml'
        config = object_value(yaml.safe_load(path.read_text()))
        if not architectures or not set(architectures).issubset(strings(config.get('arch'))):
            raise ValueError('Release platforms must be supported by the generated repository')
        config['arch'] = sorted(architectures)
        path.write_text(yaml.safe_dump(config, sort_keys=False, allow_unicode=True))


@dataclass(frozen=True, kw_only=True)
class Repository:
    """Generate store metadata for an explicit set of qualified architectures; publish nothing."""

    source: Path
    branding: Path
    output: Path
    architectures: tuple[Architecture, ...]


def generate_repository(options: Repository) -> None:
    canonical = options.source / 'apps/garmin-hass/addon'
    regular_file(canonical / 'config.yaml')
    regular_file(options.source / 'Cargo.toml')
    config = object_value(yaml.safe_load((canonical / 'config.yaml').read_text()))
    version = text(tomllib.loads((options.source / 'Cargo.toml').read_text())['workspace']['package']['version'])
    selected = options.architectures
    if not selected or len(set(selected)) != len(selected) or not set(selected).issubset(strings(config.get('arch'))):
        raise ValueError('Select distinct architectures supported by the canonical app metadata')
    if config.get('version') != version or config.get('slug') != 'garmin_toolkit':
        raise ValueError('Workspace and canonical app identity/version differ')
    if options.output.exists() or options.output.is_symlink():
        raise ValueError('App repository output already exists')
    options.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-repository.', dir=options.output.parent) as temporary:
        staged = Path(temporary) / 'repository'
        app = staged / 'garmin_toolkit'
        app.mkdir(parents=True)
        inputs = {
            'repository.yaml': canonical / 'repository.yaml',
            **{f'garmin_toolkit/{name}': canonical / name for name in ('README.md', 'DOCS.md', 'CHANGELOG.md')},
            'garmin_toolkit/icon.png': options.branding / 'icon-128.png',
            'garmin_toolkit/logo.png': options.branding / 'logo-250x100.png',
        }
        for name, origin in inputs.items():
            regular_file(origin)
            shutil.copyfile(origin, staged / name, follow_symlinks=False)
        config['arch'] = sorted(selected)
        (app / 'config.yaml').write_text(yaml.safe_dump(config, sort_keys=False, allow_unicode=True))
        check_shape(staged, 'hass-app-repository.dirspec.toml')
        staged.rename(options.output)


if __name__ == '__main__':
    import tyro

    generate_repository(tyro.cli(Repository))
