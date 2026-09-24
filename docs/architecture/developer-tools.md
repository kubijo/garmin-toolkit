# Developer tools and application logs

Desktop and HASS expose Developer tools through the icon beside Profiles, including the profile chooser. Desktop uses a
native secondary viewport; HASS opens a separate browser window with its own Rust/egui canvas. Clicking the icon again
focuses the same window; closing it and clicking again opens a replacement. If the browser blocks the popup, the app
offers Retry and Open in a tab. Logs and debug information are available without enabling automation.

The [shared window host](application-windows.md) owns popup routing, session isolation, liveness, and retry/tab
fallback. Tools has no automation driver or map renderer; highlights and canvas resizing stay in the originating app
tab. After an app reload, the old tools window retains its last report but disables commands; reopen tools to attach to
the new session.

The tools page connects directly to the existing backend log RPC, with its own filters, export, and reconnection.
Closing tools leaves the app and any running scenario alone. The floating app status overlay retains Stop and Escape
cancellation; Stop and Escape in the tools window send commands to the originating app. Pointer highlights clear when
runs complete. The status and developer Stop controls have distinct semantic IDs, `automation.stop` and
`developer.automation.stop`.

Panel sections span the available window width and share one vertical scroll area. Filters use the application's
labelled inputs and selector, with columns on wide windows and stacked fields on narrow ones. Log details expand inline
with wrapped messages and fields; copying a record retains the complete JSON. Section headers and the tools icon show
hover and keyboard-focus feedback. Maintained previews are in `infra/gallery/developer-tools.capture.toml` and
`infra/gallery/developer-header.capture.toml`. The gallery's `capture-developer-details` recipe exercises expansion and
scrolling and writes narrow/wide, dark/light captures under `.tmp/gallery/developer-interaction`.
`capture-developer-controls` captures the expanded browser hooks and resize/sequence instructions in both themes. The
`developer_panel` tests also guard compact action-row geometry on resize, narrow-window bounds, section toggling, and
pointer cursors.

## Automation and control

Demo hosts accept `--ui-automation` and `--control-server`. On desktop, `--control-server` enables automation and binds
an HTTP listener to `127.0.0.1:0`. The operating system selects the port; its actual URL is printed to stdout and shown
in Developer tools. The panel also provides explicit Start, Stop, and Copy URL controls. Production rejects automation
and control-server startup flags before opening storage. The listener is stopped on application exit.

Desktop serves `GET /api/capabilities`, `GET /api/debug`, and `POST /api/control`. The command request is JSON with an
`operation` and `argument`; responses contain either `value` or `error`. HTTP requests enter a bounded queue and the UI
thread dispatches them to the same driver as browser commands. Browser-origin requests are not accepted by the native
listener. Ordinary application launches do not bind a control port.

```json
{
  "operation": "start",
  "argument": "activity-smoke"
}
```

HASS `--control-server` enables automation and registers control routes on the existing HTTP listener. Without that
flag, control and capabilities return HTTP 404. `--ui-automation` alone enables browser hooks without HTTP control.
Production builds reject either flag. No token is required. Browser-origin HTTP requests are rejected. Control HTTP and
browser registration accept only actual loopback socket peers and localhost or loopback-IP Host headers. Forwarded
requests are rejected; control is not available through HASS ingress. Other application routes retain their configured
bind address.

`GET /api/capabilities` describes the supported operations. `POST /api/control` uses the desktop command fields. The
server automatically selects the single connected root app tab. No connected tab returns 404; multiple connected tabs
return 409 and require closing the extra tabs. Child popups do not affect selection. Session discovery and
caller-supplied session IDs are removed.

```json
{
  "operation": "start",
  "argument": "responsive-layout"
}
```

Start/action/sequence acknowledge admission with `value: null`; poll `status` or `result` for completion. HASS allocates
and returns an increasing `request_id`. Clients may supply a positive explicit ID for replay checks; it must exceed the
last admitted ID across all connections. The backend permits one outstanding command per connection, at most 16
connections, a 16 KiB request, and a 1 MiB JSON reply. Busy requests receive 429; reused or older IDs receive 409. A
five-second RPC deadline reports an unknown outcome on timeout or transport failure; commands are never retried or
redirected after admission. Browser dispatch also checks a deadline mapped to its monotonic clock.

The root browser registers a reverse typed Remoc client on its existing application connection. Each reconnect gets a
new connection ID; its page ID survives reconnect but changes on reload. Dropping the connection unregisters the session
and revokes its browser handler. Commands enter the existing browser hooks, preserving metadata, watchdog, visibility
pauses, and reports. Child popups do not register automation sessions. The endpoint is printed at startup; Developer
tools debug information shows the local endpoint. Connection and page IDs remain internal lifecycle bookkeeping.

For a local demo:

```sh
just hass::run demo "$PWD/.tmp/hass-ui-automation" --control-server
```

Open the app in a browser, then query the printed localhost endpoint. Child-window control and log access remain in the
[shared interface plan](../plans/shared-interface-workflows.md#hass-control-protocol-extension). Browser MCP and
`window.garminAutomation` remain available.

`just hass::control check` runs the four scenarios, individual action and sequence checks, cancellation after resizing,
and HTTP admission checks. Reports go to `.tmp/hass-control-runtime`; the checker never starts a server. Use
`just hass::control command status` for a single request, or `--url URL` for another localhost application address.

`screenshot` takes no argument and captures the root application on desktop and HASS. Its HTTP success response is
`image/png`, with `Cache-Control: no-store` and JSON metadata in `x-garmin-capture`: physical width/height,
`pixels_per_point`, and `requested_frame`/`received_frame`. These identify UI request and receipt, not compositor
presentation. HASS also returns `x-garmin-request-id`. Errors use the usual JSON response. Capture is limited to 8 Mi
pixels and 8 MiB of PNG data, with one pending capture and the five-second command deadline. No server-side artifact is
retained. Both ends of the HASS transport buffer up to the PNG limit plus 64 KiB of RPC/codec overhead; Remoc's default
512 KiB threshold would switch large replies to streaming that requires OS threads unavailable in browser WASM. On HASS,
save the image and metadata with:

```sh
just hass::control screenshot --output .tmp/capture.png
```

Native capture includes renderer callbacks. HASS composites the worker map underneath the UI using physical placement
and clipping. A hidden tab, pending map view, resize, or changed map during capture fails explicitly; request a new
capture after the application settles. Images exclude child windows, browser chrome, OS decorations, and system dialogs.
Screenshot capture is an HTTP operation; the existing synchronous `window.garminAutomation` hooks remain unchanged.

The panel documents these browser hooks:

- `list()`, `start(name)`, `status()`, `result()`, and `cancel()` manage scenarios.
- `targets()` lists semantic IDs, labels, roles, enabled states, values, and clipped logical bounds.
- `action({kind, target, ...})` submits one click, drag, wheel/scroll, key, or text action. Drag `x`/`y` are normalized
  target coordinates; wheel/scroll uses a logical-point `delta`; key uses an egui key name; text uses `text`.
- `action({kind: 'resize', width, height})` resizes the root view in logical egui points. Desktop resizes its native
  window; the browser Rust adapter sizes the application canvas inside the existing tab. Eframe's resize observer
  updates the backing surface and input coordinates; the worker map follows the actual canvas rectangle.
- `sequence(actions)` runs 1–64 actions in order, including resize, `wait`, `assert_available`, and `assert_value` (with
  a string `value`). All syntax is checked before starting; targets are resolved as each action executes.

`responsive-layout` selects the first demo activity, sets playback to 2×, and verifies selection, drawer access,
playback, and profile/map controls at 720 × 640 and 1100 × 720. Hidden activity rows become available through
`activity.list.toggle`; it and `activity.details.toggle` expose `open`/`closed` values. Built-in scenarios first log out
if needed, closing user-bound windows. Use a `sequence` without logout to check secondary-window state across resizing.

Resize completion requires the requested dimensions to appear in rendered input and settle for 50 ms. `status()` exposes
the pending `resize_request`; host errors and a five-second size mismatch timeout fail explicitly. Dimensions are
limited to 1–8192 logical points; native window minimum sizes still apply (currently 720 × 480). Resize steps are
functional evidence, excluded from fixed-geometry performance comparisons. Built-in scenarios restore the starting
viewport after success, failure, or cancellation; HASS restores the original canvas CSS sizing, including container
fill. Reports retain the workload geometry. Explicit `action` and `sequence` commands leave the requested size in effect
for inspection; another resize or browser reload restores the desired layout.

With the first demo activity open, [responsive-layout.json](../../infra/automation/responsive-layout.json) checks that
its selection survives narrow/wide layouts and that the profile, map-fit, and playback controls remain available. Post
that JSON to desktop `/api/control`, or pass its `argument` array to `window.garminAutomation.sequence(...)`.

An individual action uses the same input driver and reports completion asynchronously. Key/text actions first click the
target to establish focus. No command directly changes application or camera state. Missing, ambiguous, disabled, or
clipped targets fail explicitly, and overlapping workloads are rejected. Native automation targets the application's
root viewport; the developer viewport remains available for inspecting logs and stopping a workload. The driver pairs
input/output hooks using the incoming viewport ID because those hooks run outside egui's viewport pass. Secondary
windows cannot advance the workload, change its recorded geometry, or replace its semantic target tree.

Root-window resize and pixel-scale changes continue functional runs. The driver records each change, waits for fresh
layout, and resolves targets again. An active click or drag is released outside controls and retried; already applied
drag movement is not rolled back, and the interrupted attempt is retained in the report. Stationary observations restart
after map readiness returns. Runs with geometry changes are ineligible for performance comparisons.

Focus loss does not cancel automation. Hidden browser tabs pause it and safely release synthetic input before resuming
the interrupted action. Applied drag movement is not rolled back. Active-time deadlines exclude the hidden interval.
Reports distinguish completed actions, interrupted attempts, and pauses; paused runs are functional evidence only and
cannot establish uninterrupted performance acceptance. Stop, Escape, application shutdown, and actual failures still
terminate runs.

Runtime acceptance is tracked in the [shared interface plan](../plans/shared-interface-workflows.md#runtime-acceptance).

## Logs

`garmin-logging` owns native collection and persistence. Desktop, CLI, and the HASS backend install it as a tracing
layer; native event producers use a bounded writer queue. HASS browser and worker diagnostics are forwarded in bounded
batches through the same typed log service while remaining available in the browser console. Browser ingestion and
subscription errors are not recursively forwarded as log events.

The browser's Rust/WASM adapter owns record construction, session identities, source sequencing, a 512-record delivery
buffer, and batches of up to 128 records. Records remain queued until the backend acknowledges ingestion; retries retain
their original identities. The browser checks the complete serialized record against the shared 16 KiB limit, reserving
space for the backend source prefix and sequence. Oversized or invalid records are dropped with a counted warning; valid
following records continue immediately. Ingestion acknowledges valid records and reports permanent rejections
separately; these records are removed from the queue, while transport/storage-admission errors retain the batch for
retry. Worker records keep their identities when relayed through the parent. Buffer overflow emits a dropped-record
warning. JavaScript only captures browser errors, connects Rust callbacks, relays worker messages, and downloads
exports. Before WASM installs its callback, the adapter retains at most 16 bootstrap events in the page; workers forward
bootstrap errors to their parent immediately. These events receive their record identities in Rust.

Canonical records and filters live in `garmin-model`; `garmin-service-api::logging::LogService` provides history,
subscription, ingestion, and export. Desktop obtains a local Remoc client; HASS obtains a remotely transferable client
through `ApplicationService::logs`. The UI never manages backend storage paths or file formats.

Filters cover severity, component, source, session, time bounds, and text. Sequence cursors advance even over filtered
records. Resume cursors carry both a sequence and a random store epoch, regenerated whenever a store is opened. An old
epoch reports a history reset and replays retained history; the UI clears its previous log list before accepting the new
sequence space. This also handles sequence reuse after memory-only collection and restart. Browser source sequence
numbers deduplicate retries within retained history. Source identities expire when their last retained record leaves
history; a retry outside that horizon can be accepted as a new record. Expired cursors and slow-consumer gaps are
visible. Live UI history is bounded to 2,048 records. Backend files rotate at 8 MiB with four retained segments. The
query/export history also has independent limits of 16,384 records and 32 MiB of encoded records, including during
recovery and disk failures. Source bookkeeping is bounded by that history rather than a permanent admission quota.

A write or rotation failure stops persistence until the store is reopened: continuing through a partially written or
rotated file handle could corrupt retained history. Bounded live collection continues and exposes the storage error
alongside the stream. Memory eviction still produces retention gaps.

Each active store holds an OS file lock before recovering or modifying its directory. Overlapping processes use the
first free `writer-N` subdirectory, with separate histories and sequence spaces; the service's directory handle points
to its actual slot. Slots are reused after their owner exits, recovering that slot's retained history. Histories are not
merged across simultaneous processes. Dropping the last store handle drains its writer before releasing ownership.

Desktop can open the backend log folder or save a filtered JSONL export; HASS downloads an export without receiving host
paths. Exports anchor their starting cursor and ending sequence before streaming. A retention gap or storage error fails
the export rather than offering partial JSONL; its error stays visible independently of live-stream status until another
export is attempted. Existing CLI capture logs and terminal output remain available. The developer panel shows
connection status, retention gaps, structured record details, and copy controls.
