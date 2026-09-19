"""Correlate versioned browser upload marks without equating resource publication with presentation."""

import json
import math
from dataclasses import dataclass, field
from typing import Any, TypeGuard

MARK = 'garmin.map.upload'
PHASES = {'queued', 'hidden', 'visible', 'first_work', 'progress', 'published', 'first_draw', 'released'}


@dataclass
class Upload:
    upload_id: int
    tile: str
    started_trace_ms: float
    status: str = 'incomplete'
    visible_ms: float = 0.0
    hidden_ms: float = 0.0
    work_ms: float = 0.0
    bytes: int = 0
    first_work_ms: float | None = None
    published_ms: float | None = None
    first_draw_ms: float | None = None
    last_visible_to_publish_ms: float | None = None
    visibility_changes: int = 0
    error: str | None = None

    @property
    def publication_to_draw_ms(self) -> float | None:
        if self.first_draw_ms is None or self.published_ms is None:
            return None
        return self.first_draw_ms - self.published_ms

    @property
    def visible_wait_ms(self) -> float:
        """Visible queue lifetime excluding measured allocation/write CPU work, not GPU execution time."""
        return max(0.0, self.visible_ms - self.work_ms)


@dataclass
class UploadAnalysis:
    uploads: list[Upload] = field(default_factory=list)
    malformed_events: int = 0
    partial_events: int = 0


@dataclass
class _Pending:
    upload: Upload
    last_ms: float = 0.0
    visible: bool = True
    visible_since: float = 0.0

    def apply(self, detail: dict[str, Any]) -> None:
        row = self.upload
        phase, elapsed = detail['event'], detail['elapsed_ms']
        if row.status in {'drawn', 'released', 'invalid'} or elapsed < self.last_ms:
            raise ValueError('event after termination or nonmonotonic upload time')
        if row.tile != f'{detail["zoom"]}/{detail["x"]}/{detail["y"]}':
            raise ValueError('tile identity changed within an upload')
        if row.published_ms is None:
            if self.visible:
                row.visible_ms += elapsed - self.last_ms
            else:
                row.hidden_ms += elapsed - self.last_ms
        elif phase not in {'first_draw', 'released'}:
            raise ValueError('queue event after publication')

        if phase in {'hidden', 'visible'}:
            visible = phase == 'visible'
            if visible == self.visible:
                raise ValueError('duplicate visibility transition')
            self.visible = visible
            row.visibility_changes += 1
            if visible:
                self.visible_since = elapsed
        elif phase == 'first_work':
            if row.first_work_ms is not None or not self.visible:
                raise ValueError('duplicate first work or work while hidden')
            row.first_work_ms = elapsed
        elif phase == 'progress':
            if row.first_work_ms is None or not self.visible:
                raise ValueError('progress without visible work')
            row.work_ms += detail['work_ms']
            row.bytes += detail['bytes']
            if not math.isfinite(row.work_ms):
                raise ValueError('accumulated work is not finite')
        elif phase == 'published':
            if row.first_work_ms is None or not self.visible:
                raise ValueError('publication without visible work')
            if row.bytes == 0:
                raise ValueError('publication without upload progress')
            if row.work_ms > row.visible_ms + 0.001:
                raise ValueError('active work exceeds visible queue lifetime')
            row.published_ms = elapsed
            row.last_visible_to_publish_ms = elapsed - self.visible_since
            row.status = 'published'
        elif phase == 'first_draw':
            if row.published_ms is None:
                raise ValueError('draw before publication')
            row.first_draw_ms = elapsed
            row.status = 'drawn'
        elif phase == 'released':
            row.status = 'released'
        else:
            raise ValueError('unexpected queue event')
        self.last_ms = elapsed


def decode_mark_detail(event: dict[str, Any]) -> dict[str, Any]:
    """Decode Chrome's representation of JSON detail from the shared browser timing bridge."""
    args = event.get('args', {})
    if not isinstance(args, dict):
        raise ValueError('invalid mark arguments')
    data = args.get('data')
    value = args.get('detail', data.get('detail') if isinstance(data, dict) else None)
    # Chrome may JSON-encode the string supplied to PerformanceMark.detail.
    for _ in range(2):
        if isinstance(value, str):
            value = json.loads(value)
    if not isinstance(value, dict):
        raise ValueError('invalid upload detail')
    return value


def finite_number(value: Any) -> TypeGuard[int | float]:
    try:
        return type(value) in (int, float) and math.isfinite(value)
    except OverflowError:
        return False


def _validate_detail(value: dict[str, Any]) -> None:
    if (
        type(value.get('version')) is not int
        or value['version'] != 1
        or not isinstance(value.get('event'), str)
        or value.get('event') not in PHASES
    ):
        raise ValueError('unsupported upload detail')
    for key in ('upload_id', 'zoom', 'x', 'y', 'bytes'):
        if type(value.get(key)) is not int or value[key] < 0:
            raise ValueError(f'invalid {key}')
    if value['zoom'] > 30 or max(value['x'], value['y']) >= 1 << value['zoom']:
        raise ValueError('invalid tile coordinates')
    for key in ('elapsed_ms', 'work_ms'):
        if not finite_number(value.get(key)) or value[key] < 0:
            raise ValueError(f'invalid {key}')
    if value['event'] != 'progress' and (value['work_ms'] != 0 or value['bytes'] != 0):
        raise ValueError('work outside a progress event')
    if value['event'] == 'queued' and value['elapsed_ms'] != 0:
        raise ValueError('enqueue must start the upload clock')


def analyze_uploads(events: list[dict[str, Any]]) -> UploadAnalysis:
    """Input is already restricted to the selected renderer's main thread."""
    result = UploadAnalysis()
    active: dict[int, _Pending] = {}
    validated = []
    damaged_ids: set[int] = set()
    unidentified_gap = False
    for event in (item for item in events if item.get('name') == MARK):
        identity = None
        try:
            detail = decode_mark_detail(event)
            raw_identity = detail.get('upload_id')
            if type(raw_identity) is int and raw_identity >= 0:
                identity = raw_identity
            _validate_detail(detail)
            timestamp = event.get('ts')
            if not finite_number(timestamp):
                raise ValueError('invalid upload timestamp')
            validated.append((timestamp, detail))
        except ValueError, TypeError, KeyError, OverflowError:
            result.malformed_events += 1
            if identity is None:
                unidentified_gap = True
            else:
                damaged_ids.add(identity)

    for timestamp, detail in sorted(validated, key=lambda item: item[0]):
        try:
            identity = detail['upload_id']
            if detail['event'] == 'queued':
                # A navigation can reuse process and upload IDs. Keep each observed enqueue separate.
                row = Upload(identity, f'{detail["zoom"]}/{detail["x"]}/{detail["y"]}', timestamp / 1_000)
                result.uploads.append(row)
                active[identity] = _Pending(row)
            elif identity not in active:
                result.partial_events += 1
            else:
                pending = active[identity]
                if pending.upload.status == 'invalid':
                    continue
                try:
                    pending.apply(detail)
                except ValueError as error:
                    pending.upload.status = 'invalid'
                    pending.upload.error = str(error)
                    raise
        except ValueError, TypeError, KeyError:
            result.malformed_events += 1
    # Bad timestamps cannot be placed in a navigation epoch. Conservatively invalidate every
    # occurrence of the recovered ID; an unidentifiable gap invalidates the entire cohort.
    for row in result.uploads:
        if unidentified_gap or row.upload_id in damaged_ids:
            row.status = 'invalid'
            row.error = row.error or 'malformed upload event leaves lifecycle accounting incomplete'
    return result
