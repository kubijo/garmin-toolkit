# Shared interface workflows

Deliver device, FIT, route, activity, asset, and recovery workflows through the same application models and actions in
desktop and HASS. Target shells adapt transport and presentation mechanics without creating parallel behavior.

[The HASS watch slice](hass-watch-vertical-slice.md) owns hosting, ingress, packaging, and hardware deployment. This
plan owns user-visible workflow parity between its browser client and the native desktop app.

## Shared work

1. Populate each device page through automatic bounded inspection, then give it explicit FIT import, route upload, map
   management, and asset-management entry points. File transfer and mutation require separate confirmation.
2. Stage FIT ingress before persistence. Pre-parse selected or dropped files and present their activities, metadata,
   duplicates, warnings, and failures for review. Import nothing until the user explicitly confirms the staged set.
3. Confine drag and drop to visible, enabled targets. Show clear accept or reject feedback while hovering. A drop target
   cannot remain active elsewhere in the window or application.
4. Complete the [activity map workspace](activity-map-workspace.md) acceptance and rendering-isolation work without
   duplicating its implementation checklist here.
5. Report mutations through the notification host. Success follows persistence; failures remain visible and actionable
   across reconnects.
6. Prove offline sync, visualization, export, snapshot and restore, and one confirmed upload through both clients.
7. Use only original or individually licensed artwork with generated attribution.
8. Complete a keyboard-only audit of both shells, including the explorer window, focus visibility, traversal order,
   modal trapping, and reconnect overlay/recovery.
9. Complete the window, automation, and logging acceptance below. Current contracts live in
   [application windows](../architecture/application-windows.md), [Developer tools](../architecture/developer-tools.md),
   and [device explorer](../architecture/device-explorer.md). [Device inspection](device-state.md) owns hardware
   acceptance.

## Runtime acceptance

HASS/Chrome has demonstrated simultaneous tools/files windows, focus reuse, upload, download, folder creation,
responsive automation, logout closing files while retaining tools, and command isolation between two tabs. Browser
checks also cover Stop/Escape, completing a run after closing tools, text-filtered JSONL export, and log pause/resume.
With a HASS FIT Open reply held, Browse files invoked focus on the same popup without outgoing service traffic;
releasing the reply cleared busy state without changing connection generation. Viewport restoration returns to container
sizing after success, cancellation, and an injected one-shot resize failure; blocking restoration itself produces an
explicit error. Injected client transport failures reconnect the main app, files, and tools independently without extra
windows; files retain directory/selection, and logs resume with fresh records and no duplicate exported sequence IDs. A
real HASS restart also preserves popup sessions and file selection, accepts a new download request, and resumes logs
with the new server session and fresh browser records. A mock upload interrupted during transfer reconnects without
resubmitting the request or leaving a partial destination file. Both an error received before disconnect and the
interruption notice survive reconnect and Browse files refresh; late completion does not overwrite the interruption. The
HASS Refresh control retains folder/selection, blocks duplicate requests while loading and during uploads, and preserves
interruption feedback. Its layout and action work at 1000 px and 576 px browser widths. Desktop has demonstrated
simultaneous native windows and Wayland focus on one setup. Remaining work:

- **Explorer refresh:** repeat on desktop and verify recovery after a refresh error. Inspect wide and narrow scenes in
  `infra/gallery/device-explorer.capture.toml`; the rebuilt HASS notice's paragraph spacing is verified at both browser
  widths above.
- **Busy focus:** repeat during a desktop upload/download. Verify switching devices remains blocked during an operation.
- **Profile scope:** verify profile removal/switching and closure during pending operations or pickers, including FIT
  previews. Tools must stay open; late results must not restore the old user's views. Repeat ownership checks on
  desktop.
- **Window lifecycle:** verify blocked-popup retry/tab fallback and stale-popup command rejection within five seconds of
  parent reload/closure. Test real keyboard/touch focus and other Wayland compositors; closing a window while activation
  is pending must not recreate it.
- **Native decorations:** after rebuilding, check the shared title bar in tools and files: drag, double-click maximize,
  restore, minimize, close/reopen, window menu, and edge/corner resizing. Inspect the desktop Developer tools gallery
  preview in dark and light themes. Check border and shadow separation over another window, focus changes, and removal
  of the shadow surround when maximized.
- **File operations:** verify FIT preview/import in the parent, removal confirmation, picker cancellation, directory
  state across snapshots, and remaining desktop transfers. Verify no replay when the connection drops after a write
  commits but before its reply arrives.
- **Automation:** verify held-input release on cancellation, report retrieval after reopening tools, and native viewport
  restoration on success/failure/cancellation. Test resizing with files open using
  [responsive-layout.json](../../infra/automation/responsive-layout.json) after selecting the first activity: post it to
  desktop `/api/control`, or pass its `argument` to browser `sequence`. Unlike scenario startup, this does not log out.
  Child windows must preserve state and remain outside the root semantic tree.
- **Logs:** verify remaining filters, slow-consumer gaps, and desktop export. Render and inspect the Developer tools
  gallery error scene with its persistent export-failure message.

## HASS control protocol extension (next task)

Planned, not implemented. Extend the existing Rust semantic dispatcher to HASS; keep JavaScript as browser API glue.

### Contract and routing

- Extract versioned requests, replies, and capabilities from the native adapter while preserving desktop compatibility.
  Reuse `garmin_ui::automation::command`; advertise unsupported operations per session/window.
- Identify root app session, connection generation, window, and request separately. Reload creates a new session.
  Require explicit selection with multiple tabs; never retarget pending commands to another tab.
- Evaluate a reverse typed Remoc client over the existing service connection before adding WebSockets. The HASS backend
  brokers requests to the browser event loop through bounded queues with correlated replies.
- Preserve browser-hook parity: metadata, watchdog, hidden-tab pause/resume, active-time accounting, and reports
  currently wrapped by `ui-automation.js`. Move shared orchestration into Rust rather than creating a second launch
  path.
- Distinguish request acknowledgement from workload completion and expose status/results by ID. Report missing/ambiguous
  sessions, unsupported windows, queue limits, stale generations, disconnects, capture failures, and deadlines. A
  timeout after dispatch may have an unknown outcome. Reject expired queued commands; never replay input or mutations
  after reconnect. Bound pending requests and duplicate/result retention.

### Hosting

- Keep explicit enablement and the demo-build restriction. Disabling control revokes registration and rejects queued
  requests; cancellation remains separate. Desktop keeps its random loopback port.
- Choose authenticated HASS ingress or an explicitly enabled loopback endpoint before implementation. Preserve ingress
  prefixes and origin/access checks; session IDs provide routing, not authorization. Bind registration to the intended
  backend connection. Do not expose unauthenticated control through the normal HASS listener.
- Report the endpoint in stdout and Developer tools, alongside session, connection, windows, capabilities, and hooks. A
  browser must already be connected. Focus loss does not cancel; hidden tabs retain pause/report behavior.

### Screenshots and later capabilities

After root command routing, add correlated next-frame capture returning PNG or a bounded artifact handle with
session/window, frame, dimensions, and scale. Define pixel/byte limits, deadlines, cancellation, and expiry.

The pinned eframe supports capture on both targets. Native capture must include map rendering callbacks. HASS must
compose the egui canvas with the separate worker map, preserving clipping, position, transparency, and scale; reject
stale frames during resize/navigation. Missing map pixels fail acceptance. Captures exclude browser chrome, OS
decorations, and system dialogs; external automation still covers those boundaries.

Child-window control follows separately: the current driver ignores secondary viewports. Add a window registry and
scoped semantic snapshots/dispatch without mixing target trees or advancing the root workload. Invalidate user-bound
windows on profile changes. Expose existing log RPC through the adapter. An optional MCP facade can reuse these
operations and images later; browser MCP remains useful for navigation, file choosers, and console/network inspection.

### Implementation order and acceptance

1. Define the contract/session lifecycle and verify existing desktop clients.
2. Route root commands through HASS. Test two-tab targeting, independent reports, bounded admission, disabled defaults,
   ingress/access checks, and timeout/reload/reconnect without replay. Closed tabs fail pending requests.
3. Compare the same scenario/sequence through desktop HTTP, HASS HTTP, and browser hooks: reset/logout, cancellation and
   input release, resizing, focus loss, hidden-tab pauses, and report parity.
4. Verify screenshot pixels with maps/overlays, non-unit scale, clipping, resizing, pending map frames, target closure,
   byte limits, and artifact cleanup.
5. Add child-window and log access with profile/session isolation and stale-window tests; consider MCP afterward.

## Deferred Wayland activation work

Keep the working [local patch](../../vendor/winit/PATCHES.md) until a compatible upstream implementation is available.
Wayland focus is confirmed on one setup; keyboard/touch, multi-seat, and other compositors remain unverified.

Research as of 2026-09-23:

- Stable winit was `0.30.13`; `0.31` prereleases were not drop-in replacements for our eframe integration.
- [winit #3633](https://github.com/rust-windowing/winit/issues/3633) and
  [egui #8142](https://github.com/emilk/egui/issues/8142) track existing-window activation and token integration.
- [PR #2955](https://github.com/rust-windowing/winit/pull/2955) merged startup-token support.
  [PR #4612](https://github.com/rust-windowing/winit/pull/4612), adding pointer serials, was withdrawn after retesting
  its launch case; it does not establish the requirements for sibling-window focus. No matching open fix was found.

Investigate requesting a token from the source window through existing winit APIs and adding an API to activate an
existing target. This could replace global input tracking and custom expiry, but needs eframe/egui integration and real
compositor testing. Before upstreaming, recheck existing PRs, prepare a two-window reproduction, discuss the API, port
to the development branch, and seek a `0.30` backport separately. No patch has been submitted. A commit-pinned fork
could remove vendored source from this repository but would retain patch maintenance; replacing the window backend is
out of scope.

## Target edges

- Desktop composes application services in process. Native pickers remain visibly modal, and system attachment
  differences stay behind device adapters. Test Linux GIO first; test macOS and Windows before advertising them.
- HASS sends owned inputs and opaque staged or operation IDs across the typed service boundary. Browser code receives no
  host paths, storage handles, credentials, or mutation adapters. Reconnect must recover staged review and operation
  state without replaying a write.

Every interface state requires validated `garmin-ui` gallery evidence, including narrow layouts, translated prose,
rejection, partial parsing, confirmation, progress, disconnect, and recovery. HASS loader and ingress behavior also
require captured browser evidence because shared component scenes cannot prove the deployed boundary.

The OS session or HASS ingress grants process access but never identifies a toolkit user. Persist no credentials; keep
snapshots opaque and exports plaintext.

Delete this plan after desktop and HASS complete the same watch workflow through shared contracts, snapshots round-trip
between targets, advertised packages have explicit test tiers, and platform conditions remain inside adapters.
