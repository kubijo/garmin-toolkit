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
4. Report mutations through persistent notifications.
5. Prove offline sync, visualization, export, cross-target snapshot/restore, and one confirmed upload on both clients.
6. Audit keyboard navigation, focus, modal trapping, and reconnect recovery. Require licensed artwork and attribution.
7. **Add the missing profile accent-color chooser.** Persist the existing accent field; test switching/restart,
   accessibility, and both themes.
8. Restore browser Ctrl+wheel zoom over the HASS canvas. Vivaldi menu zoom works; Ctrl+wheel currently has no effect.

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
