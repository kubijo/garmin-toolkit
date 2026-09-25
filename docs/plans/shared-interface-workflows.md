# Shared interface workflows

Desktop and HASS should expose the same application models and actions. The
[HASS watch slice](hass-watch-vertical-slice.md) owns hosting and hardware deployment; this plan tracks client parity.
Contracts live in [application windows](../architecture/application-windows.md),
[Developer tools](../architecture/developer-tools.md), and [device explorer](../architecture/device-explorer.md).

## Shared work

1. Add explicit FIT import, route upload, map management, and asset entry points to inspected devices. Keep transfers
   and mutations behind separate confirmation.
2. Stage FIT files for review of activities, duplicates, warnings, and failures before confirmed import.
3. Limit drag/drop to visible, enabled targets with accept/reject feedback.
4. Finish [activity map acceptance](activity-map-workspace.md); report mutations through persistent notifications.
5. Prove offline sync, visualization, export, cross-target snapshot/restore, and one confirmed upload on both clients.
6. Audit keyboard navigation, focus, modal trapping, and reconnect recovery. Require licensed artwork and attribution.
7. **Add the missing profile accent-color chooser.** Persist the existing accent field; test switching/restart,
   accessibility, and both themes.

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
- **Automation:** desktop input release and viewport restoration on pass/failure/cancel; report retrieval after
  reopening tools; parity across HTTP and browser hooks. Use
  [responsive-layout.json](../../infra/automation/responsive-layout.json) to resize with files open without scenario
  logout.
- **Control transport:** ambiguous tabs and reconnect generations, including deadline expiry and duplicate rejection.
  Verify that admitted commands never replay or transfer to another tab.
- **Capture:** non-unit scale, clipping/transparency, pending frames, resize/navigation, child closure/reload, and
  limits.
- **Logs and diagnostics:** browser Back/Forward restoration; no app tab, multiple tabs, and app reconnect; desktop
  streaming alongside commands, server stop/restart, and export errors. Retention overflow, slow-consumer gaps, and
  disabled routes have automated coverage; live overflow and disabled-server checks remain unverified.

Gallery evidence must cover narrow layouts, translations, failures, progress, and recovery. HASS loader/ingress also
needs browser evidence. [Device inspection](device-state.md) owns hardware checks; test macOS/Windows before claiming
support beyond the Linux GIO path.

## Deferred work

Device/profile lifecycle events, historical run-ID retrieval, moving browser orchestration into Rust, and an optional
MCP facade remain separate work. Browser automation still handles browser chrome, file choosers, and console/network
inspection.

### Deferred Wayland activation work

Keep the working [winit patch](../../vendor/winit/PATCHES.md) pending a compatible upstream fix. As of 2026-09-23,
stable winit was 0.30.13 and 0.31 prereleases were incompatible with the current eframe integration.

[winit #3633](https://github.com/rust-windowing/winit/issues/3633) and
[egui #8142](https://github.com/emilk/egui/issues/8142) track activation.
[PR #2955](https://github.com/rust-windowing/winit/pull/2955) added startup tokens;
[PR #4612](https://github.com/rust-windowing/winit/pull/4612) was withdrawn and does not establish sibling-window focus.

Before upstreaming: recheck existing work, prepare a two-window reproduction, discuss an API using source-window tokens,
and test keyboard/touch, multi-seat, and multiple compositors. Port to the development branch and request a 0.30
backport separately. No patch has been submitted.

Retire this plan once both clients complete the watch workflow, snapshots round-trip, and advertised platforms have
explicit test coverage.
