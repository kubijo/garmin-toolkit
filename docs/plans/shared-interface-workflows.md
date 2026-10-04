# Shared interface workflows

Desktop and HASS share application models and actions. The [HASS watch plan](hass-watch-vertical-slice.md) owns hosting
and hardware deployment. Contracts live in [application windows](../architecture/application-windows.md),
[developer tools](../architecture/developer-tools.md), and the [device explorer](../architecture/device-explorer.md).

## Map workflow acceptance

The shared host workflow has passed desktop and packaged-HASS demo acceptance for reviewed update/removal, cancellation,
reconnect, retained outcomes, device-write exclusion, and recovery after a process stop. A paced HASS transfer continued
while the browser was disconnected and showed active progress after reconnect. Assisted recovery of an interrupted
zero-byte upload required a separate review and preserved the empty object in quarantine before verified rollback.
Neither browser reload nor host restart resumed writes without approval. The durable recovery contract and its limit are
recorded in [device recovery evidence](../research/device-capacity-and-recovery.md); host regressions live in
[`map_workflow.rs`](../../crates/garmin-services/tests/map_workflow.rs) and
[`map_update_boundary.rs`](../../crates/garmin-services/tests/map_update_boundary.rs).

Remaining: verify the workflow in the packaged add-on and on owned hardware before claiming either environment. The demo
and directory-backed tests do not establish behavior for every mounted-MTP implementation or cable disconnect.

## Shared work

- Finish route/Course and asset entry points on both clients, with review and separate approval for mutations.
- Limit drag and drop to visible, enabled targets with accept/reject feedback; report mutations through persistent
  notifications.
- Verify offline import, visualization, export, and confirmed transfer on both clients.
- Audit keyboard navigation, focus, modal trapping, translation, narrow layouts, and reconnect recovery.
- Restore browser Ctrl+wheel zoom over the HASS canvas.

The [visual refresh](visual-refresh.md) owns component styling and the deferred headless color-picker decision.

## Runtime acceptance

Use `just hass::control check` against a running demo. Remaining checks:

- **Files:** desktop FIT import, refresh/error recovery, busy focus, picker cancellation, removal confirmation, and
  snapshot directory state. Test disconnect after a write commits but before its reply. HASS selected FIT import after
  WebSocket closure is recorded in [FIT evidence](../research/fit-implementation.md#browser-import-acceptance).
- **Ownership:** profile switch/removal and window closure during operations or pickers; discard late results from old
  profiles on both clients.
- **Windows:** blocked-popup retry/tab fallback, stale popup rejection after parent reload/closure, and foreground focus
  across keyboard/touch and other Wayland compositors. Pending activation must not recreate closed windows.
- **Decorations:** native title-bar actions, window menu, resizing, borders/shadows, maximized appearance, and both
  themes.
- **Diagnostics:** file-export error recovery.

Gallery captures cover layout and translation; they do not prove live tile-provider or device transport behavior.
[Device inspection](device-state.md) owns hardware checks. Test macOS and Windows before claiming support beyond Linux.

### Deferred Wayland activation work

Keep the [winit patch](../../vendor/winit/PATCHES.md) until an upstream fix covers real pointer, keyboard, touch,
multi-seat, and compositor focus. Synthetic automation cannot supply the required input serial. Before upstreaming,
reproduce with two windows, agree on the initiating-window token API, and port to a compatible winit branch. See
[winit #3633](https://github.com/rust-windowing/winit/issues/3633) and
[egui #8142](https://github.com/emilk/egui/issues/8142).
