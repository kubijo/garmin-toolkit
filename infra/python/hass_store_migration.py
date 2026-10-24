"""Checkpoint publication history with exact leases and a durable retry bundle."""

import json
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path

from hass_publish import REPOSITORY, command, store
from hass_store import STATE_FILE, Store, oid, write_journal
from json_data import Object, object_rows, object_value, read_json, regular_file

REFS = ('before-store', 'before-state', 'after-store', 'after-state')


@dataclass(frozen=True, kw_only=True)
class CheckpointPublication:
    """Replace publication history after deploying the state-file publisher; retain Store contents."""

    store_commit: str
    state_commit: str
    output: Path


def require_idle_publication() -> None:
    pages = json.loads(
        command(
            'gh',
            'api',
            '--paginate',
            '--slurp',
            'repos/' + REPOSITORY + '/actions/workflows/hass-release.yml/runs?per_page=100',
        )
    )
    for page in object_rows(pages):
        if any(run.get('status') != 'completed' for run in object_rows(page.get('workflow_runs'))):
            raise ValueError('Finish active or queued HASS publication runs before checkpointing history')


def checkpoint_refs(report: Object) -> dict[str, str]:
    return {name: oid(report[name.replace('-', '_')]) for name in REFS}


def verify_checkpoint(owner: Store, checkout: Path, report: Object) -> None:
    refs = checkpoint_refs(report)
    old = owner.state(checkout, refs['before-state'], fetch=False)
    new = owner.state(checkout, refs['after-state'], fetch=False)
    if old.get('status') != 'qualified' or old.get('published') != refs['before-store']:
        raise ValueError('Checkpoint requires the current qualified Store metadata')
    expected: Object = {**old, 'published': refs['after-store'], 'checkpoint_of': refs['before-state']}
    tree = owner.git('rev-parse', refs['before-store'] + '^{tree}', cwd=checkout)
    if (
        new != expected
        or tree != report['metadata_tree']
        or owner.git('rev-parse', refs['after-store'] + '^{tree}', cwd=checkout) != tree
        or owner.git('show', '-s', '--format=%P', refs['after-store'], cwd=checkout) != refs['before-store']
        or owner.git('show', '-s', '--format=%P', refs['after-state'], cwd=checkout)
        or owner.git('ls-tree', '--name-only', refs['after-state'], cwd=checkout) != STATE_FILE
    ):
        raise ValueError('Checkpoint differs from the qualified state or changes Store contents')


def checkpoint_publication(options: CheckpointPublication, *, owner: Store | None = None) -> None:
    oid(options.store_commit)
    oid(options.state_commit)
    require_idle_publication()
    checkpoint_store(options, owner=owner or store())


def checkpoint_store(options: CheckpointPublication, *, owner: Store) -> None:
    """Checkpoint explicit refs; production callers enforce idle publication, drills supply a guarded transport."""
    before_store, before_state = oid(options.store_commit), oid(options.state_commit)
    identity: Object = {
        'remote': owner.remote,
        'branch': owner.branch,
        'fence': owner.fence,
        'before_store': before_store,
        'before_state': before_state,
    }
    if options.output.is_symlink():
        raise ValueError('Checkpoint output must be an owned directory')
    output = options.output.absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.hass-checkpoint.', dir=output.parent) as temporary:
        checkout = Path(temporary)
        owner.git('init', '-q', str(checkout))
        bundle = output / 'checkpoint.bundle'
        if output.exists():
            report = object_value(read_json(output / 'checkpoint.json'))
            if any(report.get(key) != value for key, value in identity.items()):
                raise ValueError('Checkpoint journal belongs to different refs or a different Store')
            regular_file(bundle)
            heads = owner.git('bundle', 'list-heads', str(bundle), cwd=checkout).splitlines()
            expected = {value + ' refs/heads/' + key for key, value in checkpoint_refs(report).items()}
            if set(heads) != expected or len(heads) != len(expected):
                raise ValueError('Checkpoint bundle refs differ from its journal')
            owner.git(
                'fetch',
                '--no-tags',
                str(bundle),
                *('refs/heads/' + name + ':refs/heads/' + name for name in REFS),
                cwd=checkout,
            )
        else:
            if (owner.head(owner.branch), owner.head(owner.fence)) != (before_store, before_state):
                raise ValueError('Store or publication state changed; inspect fresh heads before checkpointing')
            state = owner.state(checkout, before_state)
            if state.get('status') != 'qualified' or state.get('published') != before_store:
                raise ValueError('Checkpoint requires the current qualified Store metadata')
            shallow = owner.git('rev-parse', '--is-shallow-repository', cwd=checkout) == 'true'
            owner.git(
                'fetch',
                '--no-tags',
                *(['--unshallow'] if shallow else []),
                owner.remote,
                before_store,
                before_state,
                cwd=checkout,
            )
            tree = owner.git('rev-parse', before_store + '^{tree}', cwd=checkout)
            after_store = owner.commit_tree(checkout, tree, before_store, 'Checkpoint HASS Store metadata')
            after_state = owner.commit(
                checkout,
                {**state, 'published': after_store, 'checkpoint_of': before_state},
                '',
                subject='Checkpoint HASS publication state',
            )
            report = {
                **identity,
                'after_store': after_store,
                'after_state': after_state,
                'metadata_tree': tree,
                'completed': False,
            }
            verify_checkpoint(owner, checkout, report)
            for name, commit in checkpoint_refs(report).items():
                owner.git('update-ref', 'refs/heads/' + name, commit, cwd=checkout)
            output.mkdir()
            owner.git('bundle', 'create', str(bundle), '--all', cwd=checkout)
            with bundle.open('rb') as stream:
                os.fsync(stream.fileno())
            write_journal(output, report, name='checkpoint.json')
        verify_checkpoint(owner, checkout, report)
        after = oid(report['after_store']), oid(report['after_state'])
        current = owner.head(owner.branch), owner.head(owner.fence)
        if current == (before_store, before_state):
            owner.push(checkout, before_state, after[1], advertisement=(before_store, after[0]))
        elif current != after:
            raise ValueError('Store or publication state changed; refusing unrelated checkpoint updates')
        if (owner.head(owner.branch), owner.head(owner.fence)) != after:
            raise ValueError('Store or publication state changed after checkpoint publication')
        write_journal(output, {**report, 'completed': True}, replace=True, name='checkpoint.json')
