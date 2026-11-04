# Shared interface workflows

Desktop and HASS share application models and actions. Physical-device evidence lives in
[device capabilities](../research/device-capabilities.md). Current contracts live in
[application windows](../architecture/application-windows.md), [developer tools](../architecture/developer-tools.md),
and the [device explorer](../architecture/device-explorer.md).

## Shared work

- Confirm browser Ctrl+wheel page zoom in a headed browser.
- Remove the gallery-only RUSTSEC-2026-0370/0371/0372 exceptions when compatible published AccessKit/AT-SPI releases
  complete the [zbus 6 migration](https://github.com/z-galaxy/zbus/issues/2043). Preserve native accessibility; the
  [gallery audit policy](../../infra/gallery/deny.toml) rejects obsolete exceptions and other findings. AccessKit
  [defers the migration under its Rust support policy](https://github.com/AccessKit/accesskit/pull/834).
- Finish the [visual language](../architecture/visual-language.md) across controls, shells, device headers, and
  empty/offline/error states.

## Platform qualification

Before advertising Windows support, verify blocked-popup retry/tab fallback, stale popup rejection after parent
reload/closure, and foreground focus across keyboard and touch. Pending activation must not recreate closed windows.

Use pseudo-localized and long-copy gallery fixtures for layout regressions. Native child viewports do not support
automated screenshots; use inline windows for captures. Browser Ctrl+wheel requires native browser input because the
semantic wheel action injects egui input.

### Deferred gallery hot reload

Desktop hot reload is deferred. Before reusing the gallery loader, recheck the
[historical reload pauses](../research/build-performance.md#gallery-reload-measurement) on the selected version and
diagnose any recurrence. Prove callback and `tracing` lifetimes, retained profile/map/scroll/window state, compile-error
recovery, visible-frame latency, and memory across repeated Linux/macOS reloads. Incompatible type edits require
restart.

### Deferred Wayland activation work

Keep the [winit patch](../../vendor/winit/PATCHES.md) until an upstream fix covers real pointer, keyboard, touch,
multi-seat, and compositor focus. Synthetic automation cannot supply the required input serial. Before upstreaming,
reproduce with two windows, agree on the initiating-window token API, and port to a compatible winit branch. See
[winit #3633](https://github.com/rust-windowing/winit/issues/3633) and
[egui #8142](https://github.com/emilk/egui/issues/8142).
