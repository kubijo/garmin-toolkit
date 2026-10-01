# Shared interface workflows

Desktop and HASS should expose the same application models and actions. The
[HASS watch slice](hass-watch-vertical-slice.md) owns hosting and hardware deployment; this plan tracks client parity.
Contracts live in [application windows](../architecture/application-windows.md),
[Developer tools](../architecture/developer-tools.md), and [device explorer](../architecture/device-explorer.md).

## Map workflow acceptance

Desktop and HASS expose the shared host-owned map workflow described in
[presentation foundations](../architecture/presentation.md). Runtime acceptance remains before claiming packaged-host or
physical-device coverage:

Fresh desktop and HASS demo devices include the old map files advertised by the simulator. Existing devices are never
reseeded on reopen. The simulator catalog is static, so runtime removal acceptance needs a fresh demo device before
those files have been removed by an update. The shared
[workflow tests](../../crates/garmin-services/tests/map_workflow.rs) exercise removal and separate update approval using
the same fixture and directory connector as the GUI hosts.

Desktop simulation, direct updates, verified deletion followed by a separately approved update, catalog cancellation,
and retained outcomes across navigation/profile changes and a completed-operation restart have been exercised. The
restart preserved device files and returned to service consent without starting another operation. HASS demo acceptance
in Chrome also exercised simulation without source changes, verified deletion followed by a separately approved update,
catalog cancellation, and retained outcomes across browser reload and profile changes. The pending second approval
survived browser reload and a profile change without executing the update; both removed files had verified backups. A
subsequent HASS host restart preserved the completed and cancelled outcomes and all device files, returning to service
consent without starting another operation. HASS cancellation during a delayed synthetic write restored the original
file. A separate hard stop left a partial replacement and durable transaction; after restart, another profile could
explicitly recover it. Recovery restored all original file hashes and cleared the device transaction and host receipt.
Neither restart resumed writes automatically.

Rebuilt HASS cancellation cleanup passed on 2026-10-01 with Chrome minimized and unfocused. Per-run background
automation opened the catalog and submitted cancellation after a 7 MiB partial replacement write. The demo host was
briefly paused at that write, with an independent automatic-resume watchdog, so the browser could queue Cancel before
the synthetic transfer finished. Verified rollback restored all eight device files to their original hashes, cleared the
device transaction and host recovery receipt, and retained a cancelled outcome. Re-inspection returned to the catalog
without requesting recovery. Shared update/removal cleanup is also covered by the
[transaction boundary regressions](../../crates/garmin-services/tests/map_update_boundary.rs).

Desktop cancellation and interrupted-operation recovery passed on 2026-10-01. Cancellation submitted after an observed
map write restored the original Garmin files, cleared the transaction and recovery receipt, and retained a cancelled
outcome. First use also created the toolkit identity manifest. A separate process interruption left a 2 MiB partial map
and durable recovery evidence. After the user restarted the same demo data directory, the partial file remained
unchanged until Sam Runner explicitly recovered the transaction created under Alex Rider. Recovery restored all seven
pre-interruption file hashes, removed the partial replacement, cleared the device transaction and host receipt, and
returned to service consent without another recovery prompt. Further forced-shutdown tests require explicit user
agreement before stopping the running application.

The [HASS host integration test](../../apps/garmin-hass/src/devices/tests/maps.rs) drops the map client, its watch, and
the connection wrapper during an approved job, then reconnects and retries the discarded approval acknowledgement. The
same workflow completes once, and replay after completion retains the original reply. Actual browser folder creation,
deletion, and upload requests are rejected during the job; upload rejects before waiting for its body. Folder creation
and upload succeed after completion. All eight HASS device-service tests and demo-target Clippy passed.

Live HASS preparation lifecycle checks passed on 2026-10-01 in controlled, headless Chrome using per-run background
automation. Leaving the map view, switching profiles, and reopening it preserved the active preparation. Reloading the
browser and signing in again also returned to active preparation; completion waited for explicit update approval. A
file-browser upload was rejected with `another operation is modifying this device` during preparation and created no
file. After preparation released the device, an upload through Chrome's file-chooser protocol succeeded and appeared in
the browser. Probe files were removed afterward. These live checks cover preparation, which holds the same device
mutation lock; approved writes completed too quickly for the navigation check, so approved-job lifetime and lost-reply
retry coverage comes from the host integration test above.

Live desktop profile/view checks passed on 2026-10-01 after the user made the window visible. A 14-action sequence left
the map view, switched from Sam Runner to Alex Rider, and reopened the still-active preparation. Preparation finished at
explicit approval with every device file hash unchanged. A separate 13-action sequence approved the mock trail-map
update, left the view, switched back to Sam Runner, and reopened the completed outcome. Exactly one new completed
outcome appeared; only the planned old/new trail paths changed, the installed file matched the downloaded payload, and
the device transaction was cleared. The approved write finished before the view reopened, so this verifies retained
completion across navigation, not observation of an active write after reopening.

The rebuilt HASS client passed the shared-accordion smoke test on 2026-10-01. Previous outcomes, affected paths, and
operation history show a pointer cursor and open/close through the same disclosure component. Tab navigation followed by
Space and Enter closed and reopened an outcome. English/light and Czech/dark layouts were inspected at a 320 px browser
width with navigation collapsed. Review preparation supplied live path/history rows; no update was approved. The client
was returned to the catalog with all components kept, English, and its original browser size and theme.

The rebuilt desktop passed live outcome, affected-path, and operation-history expansion checks on 2026-10-01. A Czech
review at 720 × 1200 was inspected with its paths expanded; two English fallback labels found there were translated.
Native control does not expose cursor state or direct keyboard focus; those checks are covered by the shared gallery
tests and the live HASS test above. No update was approved. English, the original 1100 × 1200 window, and Keep
selections were restored afterward.

Native background control is deferred and does not block landing this workflow: fully covered desktop windows stopped
processing UI commands while diagnostics remained reachable. Requests resumed when the window became visible. The
Wayland redraw gate can prevent admission of even a `run_in_background` command; browser timer fallback does not cover
this native event-loop path. The proposed upstream renderer/event-loop patch was rejected and removed. Native acceptance
uses a visible window.

Live HASS reload during an approved retained-copy simulation passed on 2026-10-01. The browser unloaded 4 ms after the
write phase began; host diagnostics confirmed its connection closed before the first payload write. All three payloads
(30 MB) completed while disconnected and matched the planned hashes. Reopening map management showed exactly one new
completed outcome. All six source-device files retained their original hashes, and the isolated transaction was cleared.
Local evidence is in `.tmp/hass-reconnect-acceptance` (browser timing, host connection events, file hashes, verification
report, and screenshots). The 209 ms write phase completed before the new browser connection arrived; this proves write
survival and retained completion, not rendering a still-active write after reconnect. The demo host was left running
throughout.

HASS demo now accepts `--simulation-write-bytes-per-second 1000000` to pace retained-copy uploads. The host regression
observes a partial write, drops the client, reconnects to the same active operation with increased progress, and reaches
one completion without changing the source file. Stream tests cover gradual progress, payload integrity, and prompt
cancellation during pacing; transaction coverage includes rollback of a paced upload.

The paced live reload passed on 2026-10-01 using the rebuilt HASS demo. Captured transfer progress increased from 4.5 MB
before reload to 18.4 MB after reconnect, then 25.3 MB while the reopened view still showed an active operation, and
finally 30 MB. Host connection events confirm a 524 ms disconnect with writes continuing across it. All three payloads
matched their planned sizes and hashes; all six source files were unchanged. The tested run produced exactly one new
completed outcome and left no transaction debris. Screenshots, connection events, source hashes, and the verification
report are retained in `.tmp/hass-paced-reconnect`; capture `ed924edc-15fe-43d4-bdae-d67506e5fcfb` contains the host
events and simulated files. An earlier timing attempt also completed safely before the reopened view was captured. HASS
remains running on the completed view. Validation passed: 882 workspace tests, 52 gallery tests, and the full project
lint gate.

1. Retain the host regressions for active reconnect, lost approval replies, and browser-write exclusion. Complete
   packaged host restart/recovery and deployment acceptance in the HASS watch slice.
2. Retain shared gallery coverage for keyboard interaction, translated layouts, and progress visibility.
3. Perform separately authorized physical-device acceptance after synthetic and packaged-host checks pass.

The [HASS watch slice](hass-watch-vertical-slice.md) owns packaged-host delivery of this same service integration. CLI
doctor and transfer benchmarks remain separate diagnostic integration candidates after the core map workflow.

## Shared work

1. Add explicit FIT import, route upload, and asset entry points to inspected devices. Keep transfers and mutations
   behind separate confirmation.
2. Stage FIT files for review of activities, duplicates, warnings, and failures before confirmed import.
3. Limit drag/drop to visible, enabled targets with accept/reject feedback.
4. Report mutations through persistent notifications.
5. Prove offline sync, visualization, export, and one confirmed upload on both clients.
6. Audit keyboard navigation, focus, modal trapping, and reconnect recovery. Require licensed artwork and attribution.
7. Restore browser Ctrl+wheel zoom over the HASS canvas. Vivaldi menu zoom works; Ctrl+wheel currently has no effect.
8. **Deferred: headless color picker.** Fork `egui-elegance` to separate picker state and interaction logic from
   rendering. Let the application own layout, typography, semantic colors, border radii (including zero), and localized
   labels. Keep the current picker unchanged until that work is undertaken.

## Runtime acceptance

Use `just hass::control check` against a running demo. Remaining checks:

- **Files:** desktop refresh/error recovery and busy focus; remaining transfers, FIT preview/import, removal
  confirmation, picker cancellation, and snapshot directory state. Test disconnect after a write commits but before its
  reply.
- **Ownership:** profile switch/removal and window closure during operations or pickers. Keep tools open and reject late
  results for old profiles on both targets.
- **Windows:** blocked-popup retry/tab fallback, stale popup rejection after parent reload/closure, and foreground focus
  across keyboard/touch and other Wayland compositors. Pending activation must not recreate closed windows.
- **Decorations:** native title-bar actions, window menu, resizing, borders/shadows, maximized appearance, and both
  themes.
- **Diagnostics:** verify file-export error recovery.

Gallery evidence must cover narrow layouts, translations, failures, progress, and recovery. HASS loader/ingress also
needs browser evidence. [Device inspection](device-state.md) owns hardware checks; test macOS/Windows before claiming
support beyond the Linux GIO path.

### Deferred Wayland activation work

Retain the [winit patch](../../vendor/winit/PATCHES.md) until a compatible upstream fix exists. Recorded 2026-09-23:
winit 0.30.13 is compatible; 0.31 prereleases are not. Track
[winit #3633](https://github.com/rust-windowing/winit/issues/3633) and
[egui #8142](https://github.com/emilk/egui/issues/8142). Startup tokens from
[PR #2955](https://github.com/rust-windowing/winit/pull/2955) do not establish sibling-window focus. Before upstreaming:
two-window reproduction, source-window token API discussion, keyboard/touch/multi-seat/compositor checks,
development-branch port, then separate 0.30 backport request.

Other deferred work: device/profile lifecycle events, historical run retrieval, Rust browser orchestration, optional
MCP. Browser tooling retains chrome/file-picker/console/network inspection.
