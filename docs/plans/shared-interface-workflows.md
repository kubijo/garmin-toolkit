# Shared interface workflows

Desktop and HASS share application models and actions. The [HASS add-on plan](hass-watch-vertical-slice.md) owns hosting
and physical-device acceptance. Current contracts live in [application windows](../architecture/application-windows.md),
[developer tools](../architecture/developer-tools.md), and the [device explorer](../architecture/device-explorer.md).

## Shared work

- Audit keyboard navigation, focus, modal trapping, translation, narrow layouts, and reconnect recovery.
- Restore browser Ctrl+wheel zoom over the HASS canvas.
- Resolve the gallery workspace map lock timeout before relying on hot reload. Measure first visible frame and
  repeated-reload memory on Linux and macOS before claiming desktop hot reload.
- Finish the [visual language](../architecture/visual-language.md) across controls, shells, device headers, and
  empty/offline/error states. Review Gray 100/10 layers, keyboard activation, Czech and long copy, narrow overlays, and
  dark/light captures. Decide whether the headless color picker needs separate state before changing its geometry.

## Runtime acceptance

Use `just hass::control check` against a running demo where semantic control is available. Open checks:

- **Files:** desktop FIT import, refresh/error recovery, busy focus, picker cancellation, removal confirmation, and
  snapshot directory state. Test disconnect after a write commits but before its reply.
- **Ownership:** profile switch/removal and window closure during operations or pickers; discard late results from old
  profiles on both clients.
- **Windows:** before advertising Windows support, test blocked-popup retry/tab fallback, stale popup rejection after
  parent reload/closure, and foreground focus across keyboard and touch. Pending activation must not recreate closed
  windows.
- **Decorations:** native title-bar actions, window menu, resizing, borders/shadows, maximized appearance, and both
  themes.
- **Diagnostics:** file-export error recovery.
- **Localization:** use the FormatJS `en-XA` pseudo-locale and deliberately long or untranslated copy; catalog parity
  alone does not establish readable native, gallery, or browser layouts.

Extend semantic automation when a runtime check needs hover or CSS cursor state, capture while the HASS backend is
offline, or native operating-system file-picker inspection. Separate native child viewports currently advertise no
screenshot support; use the app's inline file-browser mode for visual checks until native capture is available. These
tool gaps are optional improvements for unattended checks.

### Deferred Wayland activation work

Keep the [winit patch](../../vendor/winit/PATCHES.md) until an upstream fix covers real pointer, keyboard, touch,
multi-seat, and compositor focus. Synthetic automation cannot supply the required input serial. Before upstreaming,
reproduce with two windows, agree on the initiating-window token API, and port to a compatible winit branch. See
[winit #3633](https://github.com/rust-windowing/winit/issues/3633) and
[egui #8142](https://github.com/emilk/egui/issues/8142).
