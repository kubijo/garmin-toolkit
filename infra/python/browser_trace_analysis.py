"""Headless checks for raw Chrome traces captured from the HASS client."""

import argparse
import gzip
import json
import math
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from browser_upload_analysis import UploadAnalysis, analyze_uploads, decode_mark_detail, finite_number

DEFAULT_URL_PREFIX = 'http://127.0.0.1:8099/'
FRAME_TARGET_MILLISECONDS = 1_000.0 / 60.0
STALL_LIMIT_MILLISECONDS = 33.0
WHEEL_TAIL_MICROSECONDS = 200_000.0
USER_TIMING_NAMES = (
    'garmin.map.data-admission',
    'garmin.map.tile-admission',
    'garmin.map.tile-allocation',
    'garmin.map.tile-text-reconstruction',
    'garmin.map.tile-admission-wait',
    'garmin.map.tile-admission-latency',
    'garmin.map.tile-upload-latency',
    'garmin.map.wgpu-prepare',
    'garmin.map.wgpu-draw',
    'garmin.map.worker-fallback',
)


class TraceError(RuntimeError):
    """Trace data cannot support the requested analysis."""


@dataclass(frozen=True)
class Distribution:
    count: int
    p50: float
    p95: float
    p99: float
    maximum: float

    @classmethod
    def from_values(cls, values: list[float]) -> Distribution | None:
        if not values:
            return None
        ordered = sorted(values)

        def percentile(fraction: float) -> float:
            rank = max(1, math.ceil(len(ordered) * fraction))
            return ordered[min(rank - 1, len(ordered) - 1)]

        return cls(
            count=len(ordered),
            p50=percentile(0.50),
            p95=percentile(0.95),
            p99=percentile(0.99),
            maximum=ordered[-1],
        )


@dataclass(frozen=True)
class BrowserTraceSummary:
    renderer_pid: int
    renderer_tid: int
    page_url: str
    frame_intervals: Distribution | None
    interaction_frame_intervals: Distribution | None
    frame_stalls: int
    interaction_frame_stalls: int
    blocking_microtasks: Distribution | None
    event_dispatch: dict[str, Distribution]
    user_timings: dict[str, Distribution]
    tile_requests: int
    worker_tile_requests: int
    tile_loading: TileLoading
    diagnostics: list[str]
    uploads: UploadAnalysis
    upload_telemetry: bool | None
    automation: dict[str, Any] | None


@dataclass(frozen=True)
class TileRequest:
    request_id: str
    path: str
    thread: int
    started_ms: float
    finished_ms: float | None
    cached: bool | None
    status: int | None
    failed: bool | None


@dataclass
class LoadingWindow:
    second: int
    requests: int = 0
    finishes: int = 0
    worker_replies: int = 0
    allocation_calls: int = 0
    admission_frames: int = 0
    worker_busy_ms: float = 0.0


@dataclass(frozen=True)
class TileLoading:
    request_latency: Distribution | None
    cached: int
    uncached: int
    cache_unknown: int
    failed: int
    unfinished: int
    statuses: dict[str, int]
    worker_tasks: Distribution | None
    worker_busy_ms: float
    requests: list[TileRequest]
    timeline: list[LoadingWindow]


def tile_loading(events: list[dict[str, Any]], main_tid: int) -> TileLoading:
    """Events must belong to the selected renderer. Timelines are relative to its first tile GET."""
    requests: dict[str, dict[str, Any]] = {}
    for event in sorted(events, key=lambda item: item.get('ts', 0)):
        data = event.get('args', {}).get('data', {})
        identity = data.get('requestId')
        if not isinstance(identity, str):
            continue
        name = event.get('name')
        if name == 'ResourceSendRequest' and '/map/tiles/' in data.get('url', ''):
            # Redirects keep their request identity and original start time.
            requests.setdefault(identity, {'start': event, 'response': {}, 'finish': None})
        elif identity in requests:
            if name == 'ResourceReceiveResponse':
                requests[identity]['response'] = data
            elif name == 'ResourceFinish':
                requests[identity]['finish'] = event
    origin = min((item['start']['ts'] for item in requests.values()), default=0)
    rows = []
    for identity, item in requests.items():
        start, response, finish = item['start'], item['response'], item['finish']
        rows.append(
            TileRequest(
                request_id=identity,
                path=urlsplit(start['args']['data']['url']).path,
                thread=start['tid'],
                started_ms=(start['ts'] - origin) / 1_000,
                finished_ms=(finish['ts'] - origin) / 1_000 if finish else None,
                cached=response.get('fromCache'),
                status=response.get('statusCode'),
                failed=finish['args']['data'].get('didFail') if finish else None,
            )
        )
    workers = {row.thread for row in rows if row.thread != main_tid}
    windows: dict[int, LoadingWindow] = {}

    def window(milliseconds: float) -> LoadingWindow:
        second = math.floor(milliseconds / 1_000)
        return windows.setdefault(second, LoadingWindow(second))

    for row in rows:
        window(row.started_ms).requests += 1
        if row.finished_ms is not None:
            window(row.finished_ms).finishes += 1
    tasks = []
    intervals: dict[int, list[tuple[float, float]]] = defaultdict(list)
    for event in events:
        timestamp = event.get('ts')
        if not rows or not isinstance(timestamp, int | float) or timestamp < origin:
            continue
        start = (timestamp - origin) / 1_000
        name, phase, tid = event.get('name'), event.get('ph'), event.get('tid')
        if tid in workers:
            duration = duration_milliseconds(event)
            if name == 'RunTask' and duration is not None and duration >= 0:
                tasks.append(duration)
                intervals[tid].append((start, start + duration))
            if name == 'SchedulePostMessage' and phase == 'I':
                window(start).worker_replies += 1
        elif tid == main_tid and phase in {'b', 'n', 'X'}:
            if name == 'garmin.map.tile-allocation':
                window(start).allocation_calls += 1
            elif name == 'garmin.map.tile-admission':
                window(start).admission_frames += 1
    busy = 0.0
    for spans in intervals.values():
        previous_end = -math.inf
        for start, end in sorted(spans):
            start = max(start, previous_end)
            previous_end = max(previous_end, end)
            while start < end:
                bucket = window(start)
                stop = min(end, (bucket.second + 1) * 1_000)
                duration = stop - start
                bucket.worker_busy_ms += duration
                busy += duration
                start = stop
    return TileLoading(
        request_latency=Distribution.from_values(
            [
                row.finished_ms - row.started_ms
                for row in rows
                if row.finished_ms is not None and row.finished_ms >= row.started_ms and row.failed is not True
            ]
        ),
        cached=sum(row.cached is True for row in rows),
        uncached=sum(row.cached is False for row in rows),
        cache_unknown=sum(row.cached is None for row in rows),
        failed=sum(row.failed is True for row in rows),
        unfinished=sum(row.finished_ms is None for row in rows),
        statuses=dict(Counter(str(row.status) for row in rows if row.status is not None)),
        worker_tasks=Distribution.from_values(tasks),
        worker_busy_ms=busy,
        requests=rows,
        timeline=[windows[second] for second in sorted(windows)],
    )


def load_trace(path: Path) -> list[dict[str, Any]]:
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rt') as source:
        document = json.load(source)
    events = document.get('traceEvents') if isinstance(document, dict) else None
    if not isinstance(events, list):
        raise TraceError('Chrome trace contains no traceEvents array')
    return [event for event in events if isinstance(event, dict)]


def renderer_for_url(events: list[dict[str, Any]], url_prefix: str) -> tuple[int, str]:
    candidates: list[tuple[int, str]] = []
    for event in events:
        data = event.get('args', {}).get('data', {})
        if event.get('name') == 'TracingStartedInBrowser':
            frames = data.get('frames', [])
        elif event.get('name') == 'FrameCommittedInBrowser':
            frames = [data]
        else:
            continue
        if not isinstance(frames, list):
            continue
        for frame in frames:
            if not isinstance(frame, dict):
                continue
            url = frame.get('url')
            pid = frame.get('processId')
            if (
                isinstance(url, str)
                and url.startswith(url_prefix)
                and isinstance(pid, int)
                and frame.get('isOutermostMainFrame') is True
            ):
                candidates.append((pid, url))
    if not candidates:
        raise TraceError(f'Chrome trace contains no outermost frame under {url_prefix!r}')
    unique = list(dict.fromkeys(candidates))
    if len({pid for pid, _url in unique}) != 1:
        raise TraceError(f'Chrome trace contains multiple renderer processes under {url_prefix!r}')
    return unique[0]


def renderer_main_thread(events: list[dict[str, Any]], renderer_pid: int) -> int:
    tids = {
        int(event['tid'])
        for event in events
        if event.get('ph') == 'M'
        and event.get('pid') == renderer_pid
        and event.get('name') == 'thread_name'
        and event.get('args', {}).get('name') == 'CrRendererMain'
        and isinstance(event.get('tid'), int)
    }
    if len(tids) != 1:
        raise TraceError(f'expected one CrRendererMain thread for renderer process {renderer_pid}')
    return tids.pop()


def duration_milliseconds(event: dict[str, Any]) -> float | None:
    duration = event.get('dur')
    if event.get('ph') != 'X' or not isinstance(duration, int | float) or isinstance(duration, bool):
        return None
    return float(duration) / 1_000.0


def user_timing_durations(events: list[dict[str, Any]]) -> dict[str, list[float]]:
    timings: dict[str, list[float]] = {name: [] for name in USER_TIMING_NAMES}
    pending: dict[tuple[Any, ...], float] = {}
    for event in sorted(events, key=lambda item: item.get('ts', 0)):
        name = event.get('name')
        if name not in timings:
            continue
        duration = duration_milliseconds(event)
        if duration is not None:
            timings[name].append(duration)
            continue
        phase = event.get('ph')
        timestamp = event.get('ts')
        if not isinstance(timestamp, int | float) or isinstance(timestamp, bool):
            continue
        # Chrome emits zero-duration measures as async instants.
        if phase == 'n':
            timings[name].append(0.0)
            continue
        identity = event.get('id2', event.get('id'))
        if identity is None:
            continue
        key = (event.get('pid'), event.get('tid'), event.get('cat'), name, json.dumps(identity, sort_keys=True))
        if phase == 'b':
            pending[key] = float(timestamp)
        elif phase == 'e' and key in pending:
            timings[name].append((float(timestamp) - pending.pop(key)) / 1_000.0)
    return timings


def gesture_windows(events: list[dict[str, Any]]) -> list[tuple[float, float]]:
    """Pointer contact and wheel bursts (200 ms tail), not measured camera-motion windows."""
    timed = sorted(
        (event for event in events if isinstance(event.get('ts'), int | float) and not isinstance(event['ts'], bool)),
        key=lambda event: event['ts'],
    )
    windows = []
    pointers = {}
    trace_end = 0.0
    for event in timed:
        start = float(event['ts'])
        end = start + max(0.0, (duration_milliseconds(event) or 0.0) * 1_000)
        trace_end = max(trace_end, end)
        if event.get('name') != 'EventDispatch':
            continue
        data = event.get('args', {}).get('data', {})
        kind = data.get('type')
        pointer = data.get('pointerId', 0)
        if kind == 'pointerdown':
            pointers.setdefault(pointer, start)
            windows.append((start, end))
        elif kind in {'pointerup', 'pointercancel'}:
            windows.append((pointers.pop(pointer, start), end))
        elif kind == 'wheel':
            windows.append((start, end + WHEEL_TAIL_MICROSECONDS))
    windows.extend((start, trace_end) for start in pointers.values())
    merged: list[tuple[float, float]] = []
    for start, end in sorted(windows):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    return merged


def upload_telemetry_mode(events: list[dict[str, Any]]) -> bool | None:
    configuration = [event for event in events if event.get('name') == 'garmin.map.upload-telemetry']
    if not configuration:
        return None
    if len(configuration) != 1:
        raise TraceError('Expected one startup telemetry configuration; capture one page load per trace.')
    try:
        detail = decode_mark_detail(configuration[0])
    except (ValueError, TypeError) as error:
        raise TraceError('Malformed upload telemetry configuration.') from error
    if type(detail.get('version')) is not int or detail['version'] != 1 or type(detail.get('enabled')) is not bool:
        raise TraceError('Unsupported upload telemetry configuration.')
    return detail['enabled']


def automation_report(events: list[dict[str, Any]]) -> dict[str, Any] | None:
    """Retain the built-in runner's evidence without confusing egui input with DOM events."""
    starts = [event for event in events if event.get('name') == 'garmin.automation.start']
    phases = [event for event in events if event.get('name') == 'garmin.automation.phase']
    if not starts and not phases:
        return None
    if len(starts) != 1:
        raise TraceError('Expected one automation start per trace.')
    try:
        name = decode_mark_detail(starts[0])['name']
        if not isinstance(name, str) or not name or len(name) > 80:
            raise ValueError('invalid scenario name')
        reports = [decode_mark_detail(event) for event in sorted(phases, key=lambda event: event.get('ts', 0))]
        for report in reports:
            if (
                type(report.get('version')) is not int
                or report['version'] not in {1, 2}
                or report.get('scenario') != name
            ):
                raise ValueError('unsupported or mismatched scenario')
        terminals = [report for report in reports if report.get('state') not in {'running', 'paused'}]
        if not terminals:
            return {**(reports[-1] if reports else {}), 'scenario': name, 'state': 'incomplete'}
        if len(terminals) != 1 or reports[-1] is not terminals[0]:
            raise ValueError('multiple terminals or activity after termination')
        report = terminals[0]
        if report['state'] not in {'passed', 'failed', 'cancelled'}:
            raise ValueError('unknown outcome')
        if report['state'] == 'passed':
            total = report.get('total')
            actions = report.get('actions')
            if (
                type(total) is not int
                or not 0 < total <= 128
                or type(report.get('completed')) is not int
                or report['completed'] != total
                or not isinstance(actions, list)
                or len(actions) != total
            ):
                raise ValueError('passing report has incomplete workload')
            for action in actions:
                if not isinstance(action, dict) or any(
                    not finite_number(action.get(key)) or action[key] < 0
                    for key in ['scheduled_seconds', 'actual_seconds', 'lateness_seconds']
                ):
                    raise ValueError('invalid action timing')
        if report.get('version') == 2:
            pauses = report.get('pauses')
            if not isinstance(pauses, list) or type(report.get('performance_eligible')) is not bool:
                raise ValueError('invalid pause evidence')
            for pause in pauses:
                if not isinstance(pause, dict) or any(
                    not finite_number(pause.get(key)) or pause[key] < 0
                    for key in ['started_seconds', 'duration_seconds']
                ):
                    raise ValueError('invalid pause interval')
            if pauses and report['performance_eligible']:
                raise ValueError('paused run claimed uninterrupted performance eligibility')
            if report.get('viewport_changes') and report['performance_eligible']:
                raise ValueError('resized run claimed uninterrupted performance eligibility')
        return report
    except (KeyError, ValueError, TypeError) as error:
        raise TraceError(f'Malformed automation evidence: {error}') from error


def analyze_trace(events: list[dict[str, Any]], url_prefix: str = DEFAULT_URL_PREFIX) -> BrowserTraceSummary:
    renderer_pid, page_url = renderer_for_url(events, url_prefix)
    renderer_tid = renderer_main_thread(events, renderer_pid)
    renderer = [event for event in events if event.get('pid') == renderer_pid]
    raw_main = [event for event in renderer if event.get('tid') == renderer_tid]
    upload_telemetry = upload_telemetry_mode(raw_main)
    automation = automation_report(raw_main)
    timed = [event for event in renderer if finite_number(event.get('ts'))]
    main = [event for event in timed if event.get('tid') == renderer_tid]
    loading = tile_loading(timed, renderer_tid)

    frame_times = sorted(
        float(event['ts'])
        for event in main
        if event.get('name') == 'BeginMainThreadFrame'
        and isinstance(event.get('ts'), int | float)
        and not isinstance(event['ts'], bool)
    )
    frame_intervals = [(right - left) / 1_000.0 for left, right in zip(frame_times, frame_times[1:], strict=False)]
    windows = iter(gesture_windows(main))
    window = next(windows, None)
    interaction_frame_intervals = []
    for left, right in zip(frame_times, frame_times[1:], strict=False):
        while window is not None and window[1] < left:
            window = next(windows, None)
        if window is not None and right > window[0]:
            interaction_frame_intervals.append((right - left) / 1_000.0)
    microtasks = [
        duration
        for event in main
        if event.get('name') == 'RunMicrotasks'
        and (duration := duration_milliseconds(event)) is not None
        and duration > FRAME_TARGET_MILLISECONDS
    ]

    dispatch: dict[str, list[float]] = {}
    timings = user_timing_durations(main)
    tile_requests = 0
    worker_tile_requests = 0
    for event in main:
        duration = duration_milliseconds(event)
        if event.get('name') == 'EventDispatch' and duration is not None:
            event_type = event.get('args', {}).get('data', {}).get('type')
            if isinstance(event_type, str):
                dispatch.setdefault(event_type, []).append(duration)
    for event in events:
        if event.get('pid') != renderer_pid:
            continue
        if event.get('name') == 'ResourceSendRequest':
            url = event.get('args', {}).get('data', {}).get('url')
            if isinstance(url, str) and '/map/tiles/' in url:
                if event.get('tid') == renderer_tid:
                    tile_requests += 1
                else:
                    worker_tile_requests += 1

    diagnostics = []
    if automation is not None:
        diagnostics.append('Scripted egui input bypasses DOM events; DOM interaction-frame statistics do not cover it.')
        if automation.get('performance_eligible') is False:
            diagnostics.append(
                'Automation was interrupted or resized; '
                'this run is ineligible for uninterrupted performance comparisons.'
            )
        if automation['state'] != 'passed':
            diagnostics.append(f'Automation did not pass: {automation["state"]}.')
    # Keep raw upload marks for lifecycle invalidation, but never sort malformed
    # timestamps in frame, gesture, or network analysis. Metadata need not be timed.
    uploads = analyze_uploads(raw_main)
    invalid_timestamps = sum(event.get('ph') != 'M' and not finite_number(event.get('ts')) for event in renderer)
    if invalid_timestamps:
        diagnostics.append(f'{invalid_timestamps} events have invalid timestamps; timing coverage is incomplete.')
    if upload_telemetry is False and (uploads.uploads or uploads.malformed_events or uploads.partial_events):
        raise TraceError('Upload lifecycle events were captured despite disabled telemetry.')
    if not uploads.uploads and upload_telemetry is not False:
        diagnostics.append(
            'No correlated upload lifecycles; visible waiting cannot be separated from offscreen retention.'
        )
    if uploads.malformed_events or uploads.partial_events:
        diagnostics.append(
            f'Upload lifecycle gaps: {uploads.malformed_events} malformed and {uploads.partial_events} partial events.'
        )
    if len(loading.requests) != tile_requests + worker_tile_requests:
        diagnostics.append(
            'Some tile GETs lack request identities or repeat them; latency covers matched identities only.'
        )
    if timings['garmin.map.worker-fallback']:
        diagnostics.append('Worker fallback was used; this capture does not validate worker-only performance.')
    if tile_requests:
        diagnostics.append(f'{tile_requests} tile requests originated on the main thread; inspect worker fallback.')
    if not timings['garmin.map.tile-admission']:
        diagnostics.append('No tile-admission timing was captured; bounded admission is unverified.')
    if not timings['garmin.map.tile-allocation']:
        diagnostics.append('No tile-allocation timing was captured; destination allocation cost is unverified.')
    if not timings['garmin.map.tile-admission-wait'] or not timings['garmin.map.tile-upload-latency']:
        diagnostics.append(
            'Tile lifecycle markers are missing; admission wait and upload latency are not fully measured.'
        )

    return BrowserTraceSummary(
        renderer_pid=renderer_pid,
        renderer_tid=renderer_tid,
        page_url=page_url,
        frame_intervals=Distribution.from_values(frame_intervals),
        interaction_frame_intervals=Distribution.from_values(interaction_frame_intervals),
        frame_stalls=sum(interval > STALL_LIMIT_MILLISECONDS for interval in frame_intervals),
        interaction_frame_stalls=sum(interval > STALL_LIMIT_MILLISECONDS for interval in interaction_frame_intervals),
        blocking_microtasks=Distribution.from_values(microtasks),
        event_dispatch={
            name: distribution
            for name, values in dispatch.items()
            if (distribution := Distribution.from_values(values))
        },
        user_timings={
            name: distribution for name, values in timings.items() if (distribution := Distribution.from_values(values))
        },
        tile_requests=tile_requests,
        worker_tile_requests=worker_tile_requests,
        tile_loading=loading,
        diagnostics=diagnostics,
        uploads=uploads,
        upload_telemetry=upload_telemetry,
        automation=automation,
    )


def format_distribution(value: Distribution | None) -> str:
    if value is None:
        return 'none'
    return (
        f'n={value.count} p50={value.p50:.2f} ms p95={value.p95:.2f} ms '
        f'p99={value.p99:.2f} ms max={value.maximum:.2f} ms'
    )


def print_summary(summary: BrowserTraceSummary, *, timeline: bool = False) -> None:
    print('Browser map trace')
    print(f'  page       {summary.page_url}')
    print(f'  renderer   pid {summary.renderer_pid}, tid {summary.renderer_tid}')
    mode = {True: 'on', False: 'off', None: 'not recorded'}[summary.upload_telemetry]
    print(f'  upload telemetry {mode}')
    if summary.automation is not None:
        run = summary.automation
        print(f'  automation {run["scenario"]}: {run["state"]} ({run.get("completed", 0)}/{run.get("total", "?")})')
    print(f'  interaction {format_distribution(summary.interaction_frame_intervals)}')
    print('              pointer contact / wheel bursts + 200 ms; overlapping frame intervals, not presentation FPS')
    print(f'  interact >33 ms {summary.interaction_frame_stalls}')
    print(f'  all frames  {format_distribution(summary.frame_intervals)}')
    print(f'  all >33 ms  {summary.frame_stalls}')
    print(f'  microtasks {format_distribution(summary.blocking_microtasks)}')
    print(f'  tile GETs  main {summary.tile_requests}, worker {summary.worker_tile_requests}')
    loading = summary.tile_loading
    print(f'  tile latency {format_distribution(loading.request_latency)}')
    print('              request-to-finish trace events; includes browser/worker delivery delay, not just wire time')
    print(f'  tile cache cached {loading.cached}, uncached {loading.uncached}, unknown {loading.cache_unknown}')
    print(f'  tile HTTP  {loading.statuses}; transport failures {loading.failed}, unfinished {loading.unfinished}')
    print(f'  worker tasks {format_distribution(loading.worker_tasks)}; busy wall time {loading.worker_busy_ms:.2f} ms')
    for diagnostic in summary.diagnostics:
        print(f'  WARNING: {diagnostic}')
    if summary.user_timings:
        print('\nMap User Timing')
        for name, distribution in summary.user_timings.items():
            print(f'  {name:<39} {format_distribution(distribution)}')
    if summary.event_dispatch:
        print('\nInput dispatch')
        for name, distribution in sorted(summary.event_dispatch.items()):
            print(f'  {name:<16} {format_distribution(distribution)}')
    if summary.uploads.uploads:
        rows = summary.uploads.uploads
        print('\nCorrelated upload lifecycles (publication and draw submission, not presentation)')
        print(f'  states {dict(Counter(row.status for row in rows))}')
        complete = [row for row in rows if row.status in {'drawn', 'published'}]
        for label, values in (
            ('visible queue lifetime', [row.visible_ms for row in complete]),
            ('visible wait excluding CPU', [row.visible_wait_ms for row in complete]),
            ('offscreen retention', [row.hidden_ms for row in complete]),
            ('active CPU work', [row.work_ms for row in complete]),
            ('last visible to publish', [row.last_visible_to_publish_ms for row in complete]),
            ('publication to first draw', [row.publication_to_draw_ms for row in complete]),
        ):
            print(
                f'  {label:<26} {format_distribution(Distribution.from_values([v for v in values if v is not None]))}'
            )
        print(
            '  Slowest completed uploads: id tile total-ms visible-ms hidden-ms work-ms last-visible-ms draw-delay-ms'
        )
        for row in sorted(complete, key=lambda row: row.published_ms or 0, reverse=True)[:10]:
            draw = 'pending' if row.publication_to_draw_ms is None else f'{row.publication_to_draw_ms:.2f}'
            print(
                f'  {row.upload_id} {row.tile} {row.published_ms:.2f} {row.visible_ms:.2f} {row.hidden_ms:.2f}'
                f' {row.work_ms:.2f} {row.last_visible_to_publish_ms:.2f} {draw}'
            )
        print(
            '  Visible lifetime includes active work and frame scheduling; incomplete/released rows are excluded above.'
        )
    if timeline:
        print('\nLoading timeline (seconds from first tile GET; inactive windows omitted)')
        print('  second   GETs finishes replies alloc-calls admission-frames worker-busy-ms')
        for row in loading.timeline:
            print(
                f'  {row.second:6} {row.requests:6} {row.finishes:8} {row.worker_replies:7}'
                f' {row.allocation_calls:11} {row.admission_frames:16} {row.worker_busy_ms:14.2f}'
            )
        print('  Replies include route/label results; allocation calls are not tile counts.')
        print(
            '  Per-stage latency measures are separate above; end-to-end visibility requires correlated tile markers.'
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', type=Path)
    parser.add_argument('--url-prefix', default=DEFAULT_URL_PREFIX)
    parser.add_argument('--json', action='store_true', dest='as_json')
    parser.add_argument('--timeline', action='store_true', help='show loading activity in one-second windows')
    arguments = parser.parse_args()
    try:
        summary = analyze_trace(load_trace(arguments.trace), arguments.url_prefix)
    except (OSError, json.JSONDecodeError, TraceError) as error:
        raise SystemExit(f'error: {error}') from error
    if arguments.as_json:
        print(json.dumps(asdict(summary), indent=2, sort_keys=True))
    else:
        print_summary(summary, timeline=arguments.timeline)


if __name__ == '__main__':
    main()
