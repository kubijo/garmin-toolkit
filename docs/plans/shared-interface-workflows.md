# Shared interface workflows

Deliver device, FIT, route, activity, asset, and recovery workflows through the same application models and actions in
desktop and HASS. Target shells adapt transport and presentation mechanics without creating parallel behavior.

[The HASS watch slice](hass-watch-vertical-slice.md) owns hosting, ingress, packaging, and hardware deployment. This
plan owns user-visible workflow parity between its browser client and the native desktop app.

## Shared work

01. Populate each device page through automatic bounded inspection, then give it explicit FIT import, route upload, map
    management, and asset-management entry points. File transfer and mutation require separate confirmation.
02. Stage FIT ingress before persistence. Pre-parse selected or dropped files and present their activities, metadata,
    duplicates, warnings, and failures for review. Import nothing until the user explicitly confirms the staged set.
03. Confine drag and drop to visible, enabled targets. Show clear accept or reject feedback while hovering. A drop
    target cannot remain active elsewhere in the window or application.
04. Complete the [activity map workspace](activity-map-workspace.md) acceptance and rendering-isolation work without
    duplicating its implementation checklist here.
05. Report mutations through the notification host. Success follows persistence; failures remain visible and actionable
    across reconnects.
06. Prove offline sync, visualization, export, snapshot and restore, and one confirmed upload through both clients.
07. Use only original or individually licensed artwork with generated attribution.
08. Complete a keyboard-only audit of both shells, including the explorer window, focus visibility, traversal order,
    modal trapping, and reconnect overlay/recovery.
09. Complete the window, automation, and logging acceptance below. Current contracts live in
    [application windows](../architecture/application-windows.md),
    [Developer tools](../architecture/developer-tools.md), and [device explorer](../architecture/device-explorer.md).
    [Device inspection](device-state.md) owns hardware acceptance.
10. **Missing profile accent-color chooser:** profile settings display the stored accent but provide no way to change
    it. Add a shared chooser for desktop and HASS, persist the selection in the existing profile accent field, and
    verify it survives profile switching and restart, with accessible controls in both themes.

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

## HASS control protocol extension

Root command forwarding uses the existing HASS HTTP listener and a reverse Remoc client on each browser connection. The
[endpoint contract](../architecture/developer-tools.md#automation-and-control) is shared with desktop. Use
`just hass::control check` for direct localhost HTTP runtime acceptance. Complete the localhost isolation and
fault-injected transport cases below before claiming deployed HASS parity.

### Contract and routing

- Verify request/reply parity through desktop HTTP, HASS HTTP, and browser hooks. HASS currently uses the existing
  JavaScript orchestration to preserve metadata, watchdog, and visibility behavior; moving that orchestration into Rust
  remains separate work.
- Reports currently describe the session's latest workload, matching the existing driver. Add bounded historical run-ID
  retrieval before supporting clients that need to retain several completed reports remotely.
- Check ambiguous-tab rejection, reconnect generations, closed-tab revocation, deadline expiry, increasing request IDs,
  and rejection of duplicate commands after timeout. Admitted requests must never replay or move to another connection.

### Hosting

- Check that all control routes return 404 unless explicitly enabled, and that production rejects enablement.
- Verify loopback socket peers, localhost Host headers, and origin/forwarded-request rejection on the existing HASS
  listener. Control is unavailable through ingress. Desktop retains its random loopback port and existing client
  contract.
- Verify automatic single-tab selection and Developer tools debug information after reload/reconnect. A browser must
  already be connected. Focus loss must not cancel; hidden tabs retain pause/report behavior.

### Screenshots and later capabilities

Root screenshot capture returns bounded PNG bytes with frame, dimensions, and scale through the HTTP adapters. HASS
capture has demonstrated the activity map, route, overlays, and sidebars at unit scale, with another control request
succeeding after capture. Use `just hass::control screenshot --output .tmp/capture.png`.

Native root capture has demonstrated the activity map, route, and overlays after stationary arrival. Repeat capture at
non-unit scale and during resize/navigation, checking clipping, transparency, and rejection of stale frames. Missing map
pixels fail acceptance. Captures exclude browser chrome, OS decorations, and system dialogs; external automation still
covers those boundaries.

Child-window control has demonstrated discovery and separate tools/files PNG captures in HASS/Vivaldi through HTTP,
without browser MCP. File refresh and resizing to 720 × 640 preserve the root report; cancelling a child action leaves
an active root workload running. Closing/reopening files expires the old handle, and logout revokes files while
retaining tools. Root-only scenarios reject child selectors. Vivaldi required popup permission before synthetic clicks
could open windows.

Desktop has also demonstrated file-window discovery, refresh, resizing to 720 × 640, root/child cancellation isolation,
fresh handles after reopen, and logout revocation while retaining tools. Native tools section targets can toggle
Automation, Control, Logs, and Debug without changing the root report. Repeated rapid Control collapse → Debug click
sequences pass with target-geometry stabilization; subsequent HTTP commands respond while the root report stays
unchanged. Verify browser closure/reload during capture, non-unit scale, and actual foreground focus; acceptance of a
focus request alone does not prove the browser/compositor granted it. Native child capture remains deferred because
eframe's immediate viewport renderer does not process screenshot requests; its explicit unsupported response is
verified. No additional upstream patch is carried for this feature.

Next, expose existing log RPC through the adapter. An optional MCP facade can reuse these operations and images later;
browser MCP remains useful for navigation, file choosers, and console/network inspection.

### Implementation order and acceptance

1. Verify the shared contract/session lifecycle and existing desktop clients.
2. Route root commands through HASS. Test ambiguous-tab rejection, bounded admission, disabled defaults,
   localhost/access checks, and timeout/reload/reconnect without replay. Closed tabs fail pending requests.
3. Compare the same scenario/sequence through desktop HTTP, HASS HTTP, and browser hooks: reset/logout, cancellation and
   input release, resizing, focus loss, hidden-tab pauses, and report parity.
4. Verify screenshot pixels with maps/overlays, non-unit scale, clipping, resizing, pending map frames, target closure,
   byte limits, and artifact cleanup.
5. Complete child-window runtime acceptance, then add log access with profile/session isolation; consider MCP afterward.

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
