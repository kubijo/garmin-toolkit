# Developer tools and application logs

Developer tools opens beside the app through the [shared window host](application-windows.md). Reopening focuses the
existing window. Logs and debug information work without automation. Closing tools leaves scenarios running; Stop or
Escape cancels them. After a parent reload, reopen tools to attach to the new session.

## Automation and control

Demo builds accept `--control-server`, which also enables automation. Desktop binds a random loopback port and prints
its URL; HASS uses its existing listener. Without the flag, desktop does not listen and HASS returns 404 for control and
diagnostic routes. `--ui-automation` alone enables automation without HTTP access. Production rejects both flags.

Access requires a loopback socket peer and localhost/loopback Host. Forwarded requests and browser-origin commands are
rejected; diagnostic reads allow same-origin browsers. No token is required. HASS ingress cannot access control.

```sh
just hass::run demo "$PWD/.tmp/hass-ui-automation" --control-server
just hass::control command status
just hass::control check
```

The checker writes to `.tmp/hass-control-runtime`. Pass `--url URL` to target another running localhost listener.

| Endpoint                | Contract                                                             |
| ----------------------- | -------------------------------------------------------------------- |
| `GET /api/capabilities` | Supported operations                                                 |
| `GET /api/debug`        | Desktop debug information                                            |
| `POST /api/control`     | JSON command; JSON `value` or `error`, except PNG screenshot success |

```json
{
  "operation": "start",
  "argument": "activity-smoke"
}
```

Start/action/sequence acknowledge admission with `value: null`; poll `status` or `result` for completion. HASS selects
the sole connected root tab: none returns 404, multiple tabs return 409. Child windows do not affect selection.
Disconnect revokes the root's reverse Remoc client; admitted commands never replay or move to another connection.

HASS allows 16 connections, one pending command per connection, 16 KiB requests, and 1 MiB JSON replies. It returns an
increasing `request_id`; caller-supplied IDs must exceed the last admitted ID. Busy requests return 429; reused IDs
return 409. The five-second deadline reports an unknown outcome when completion cannot be confirmed.

### Child windows

`windows` returns `id`, `kind`, `title`, `ready`, `focused`, and `screenshots`. Put an ID in the command's `window`
field; omission or `root` selects the app. Handles expire on close, owner logout, or popup reload. Stale handles fail
instead of selecting another window.

`window.focus` and `window.close` require a child handle and no argument. Focus remains subject to browser/compositor
policy. Each window has its own targets, actions, sequences, status, results, and cancellation. Named scenarios run only
in the root.

```sh
just hass::control command windows
just hass::control command targets --window HANDLE
just hass::control command action --window HANDLE --argument '{"kind":"click","target":"files.refresh"}'
just hass::control screenshot --window HANDLE --output .tmp/files.png
```

Developer section targets are `developer.section.{automation,control,logs,debug,log-time}`; record expanders use
`developer.section.record.<sequence>`. Sections report `open`/`closed`. Root and tools Stop buttons use
`automation.stop` and `developer.automation.stop`.

### Screenshots

`screenshot` takes no argument. Success returns `image/png` and `x-garmin-capture` metadata: dimensions,
`pixels_per_point`, and requested/received UI frames. HASS also returns `x-garmin-request-id`. Limits are 8 Mi pixels, 8
MiB PNG, one pending capture, and the command deadline.

Captures include maps and overlays within one canvas, excluding browser chrome and system dialogs. Hidden tabs, pending
map views, resize, navigation, and child closure can invalidate a capture. Retry after the view settles. HASS captures
child canvases; native immediate viewports advertise `screenshots: false` because eframe does not process their capture
requests. Native root capture works.

### Semantic actions

The browser exposes `window.garminAutomation`; HTTP commands use the same driver.

| Operation                                     | Input or result                                                         |
| --------------------------------------------- | ----------------------------------------------------------------------- |
| `list`, `start`, `status`, `result`, `cancel` | Discover, run, inspect, and stop scenarios                              |
| `targets`                                     | Semantic IDs, labels, roles, enabled states, values, and clipped bounds |
| `action`                                      | `kind` and `target`; click, drag, scroll, key, text, or resize          |
| `sequence`                                    | 1–64 actions; also supports wait, assert_available, and assert_value    |

Drag coordinates are normalized to the target; scroll deltas use logical points; keys use egui names. Text uses `text`;
assertions use a string `value`. Resize uses `width`/`height` in logical points (1–8192), subject to native minimum
sizes. Browser resize changes the canvas, not browser chrome.

Targets resolve at execution time. Pointer actions require stable bounds across two frames within two seconds. Key/text
actions establish focus first. Missing, ambiguous, disabled, or clipped targets fail; each window permits one workload
at a time.

Named scenarios log out when needed and restore their initial viewport on success, failure, or cancellation. Explicit
actions/sequences leave their final size in place. Resize waits for the rendered size to settle for 50 ms, with a
five-second timeout. `status` exposes `resize_request`.

`responsive-layout` tests selection, drawers, playback, and controls at narrow/wide sizes. Use
[responsive-layout.json](../../infra/automation/responsive-layout.json) after selecting the first activity to test
resizing without logout or closing user-bound windows.

Focus loss does not cancel. Hidden tabs pause active-time deadlines and release synthetic input. Geometry changes
release and retry interrupted gestures after layout settles; prior drag movement is not rolled back. Reports retain
pauses, interrupted attempts, and geometry changes. These runs are functional evidence, not uninterrupted performance
measurements.

## Read-only diagnostics

These routes share the control server's enablement and access rules, with a separate allowance of 32 streams.

| Route                                   | Response                                                       |
| --------------------------------------- | -------------------------------------------------------------- |
| `/api/logs-get`, `/api/logs-stream`     | Recent application logs; stream continues with new records     |
| `/api/events-get`, `/api/events-stream` | Current state and recent events; stream continues with changes |

Events cover connections, windows, automation, and renderers across all connected browsers. Reads work without an app
tab. History retains at most 2,048 records/8 MiB; intermediate progress may be coalesced. Closing a window removes its
current state. Subscription failure clears stale telemetry and marks collection unavailable; the app stays connected.
Source overflow reports incomplete state.

| Filters                                            | Applies to     |
| -------------------------------------------------- | -------------- |
| `source`, `since_ms`, `until_ms`, `after`, `limit` | Both resources |
| `minimum`, `component`, `text`                     | Logs           |
| `kind`, `window`                                   | Events         |

Limits default to 100 records, maximum 256. Event kind/window matches are exact; log matching uses the existing filter.
Time bounds are inclusive Unix milliseconds and affect history only. Invalid, unknown, or duplicate filters return 400.

Without `after`, reads return the newest matches in chronological order. Replies contain `cursor`, `more`, and `gap`;
events add `state` and `snapshot_revision`. State stays current while history pages forward. Cursors advance over
nonmatches. Restart, retention loss, and source-history loss signal gaps even in filtered views. SSE reconnect prefers
`Last-Event-ID` over the URL cursor.

Open these routes directly in a browser for HTML, or use curl for readable text. Override with `format=text`, `ansi`,
`html`, `json` (get), or `sse` (stream). Rust/Askama formats the content; Axum constructs SSE. Streams suppress
unchanged content and advance past nonmatching records; SSE sends keepalives. HASS stops diagnostic streams before
waiting for HTTP connections to drain on shutdown.

```sh
just hass::diagnostics logs --minimum warn --follow
just cli::run diagnostics --url http://127.0.0.1:35435 events --json
```

CLI `--json` selects JSON snapshots or SSE with `--follow`. `--after` resumes a cursor, `--output PATH` saves the
response, and Ctrl-C stops following. Terminal output requests ANSI colors according to `--color`, `NO_COLOR`, and
`FORCE_COLOR`; files and pipes default to plain text.

## Logs

`garmin-logging` owns collection and persistence. `LogService` exposes history, subscription, ingestion, and export to
desktop and HASS, including browser/worker records. Backend paths stay private; transport failures are not logged
through the failing transport.

Browser ingestion retains record identities across retries. Permanent rejections are counted and discarded;
unacknowledged records remain queued. Deduplication lasts only while the original record remains in retained history.
Overflow and resume gaps are reported.

| Bound                   | Limit                              |
| ----------------------- | ---------------------------------- |
| Record                  | 16 KiB, including backend metadata |
| Browser queue / batch   | 512 / 128 records                  |
| Backend query history   | 16,384 records and 32 MiB          |
| Developer panel history | 2,048 records                      |
| Persistent segments     | Four at 8 MiB each                 |

Store epochs distinguish restarts from sequence continuation. Write/rotation failure stops persistence until reopening,
while bounded live collection continues with an error. Concurrent writers use locked `writer-N` directories with
independent histories; dropping a store drains its writer before releasing ownership.

Desktop saves filtered JSONL exports; HASS downloads them. Exports fix their start/end cursor and fail on gaps or
storage errors rather than returning partial data. Export errors remain visible until another attempt.

Outstanding runtime checks live in the
[shared interface plan](../plans/shared-interface-workflows.md#runtime-acceptance).
