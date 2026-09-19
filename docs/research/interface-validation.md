# Interface validation evidence

This consolidates the 2026-09-13/14 visual-refresh and device-explorer records. Their exact source revisions were not
recorded; these are historical observations, not a claim that the current diff passes. Temporary captures may need
regeneration from the maintained recipes. Open work belongs to [visual refresh](../plans/visual-refresh.md),
[shared workflows](../plans/shared-interface-workflows.md), and [device inspection](../plans/device-state.md).

## Shared visuals and shell

The inspected `visual-language.capture.toml` and component captures covered square controls and surfaces, flush select
and profile menus, neutral selection colors, unchanged validation-border width, docked modal actions, centered rows,
square full-height toast rails, and preserved circular avatars. Both themes were inspected at native resolution under
`.tmp/gallery/visual-polish/`. Detailed dimensions belong to production primitives and the
[visual language](../architecture/visual-language.md), not this capture history.

The 2026-09-14 `.tmp/gallery/stateful-nav-toggle/` render covered a 32-pixel native title bar and collapsed navigation
rail, square window controls, and stateful navigation toggles. Rail and Narrow states are maintained capture cases. At
that point, 63 shared-UI tests and strict all-feature Clippy passed.

Gallery canvas containment and theme-aware stage/checkerboard contrast were verified against upstream fixes and then
released dependencies. There is no remaining adjacent-checkout workaround to adopt. Foreground popups require explicit
viewport clipping and the invoking pane's palette; ambient egui popup styling can escape a gallery's scoped theme.

## Explorer and notifications

`device-explorer.capture.toml` covers the integrated window, narrow drawers, storage root, deep folders, generic and FIT
selections, empty storage, inspection failure, both themes, and Czech. `components.capture.toml` covers tree/list
alignment, mixed-height notifications, and a 360-pixel toast stack. Keyboard regression coverage injects a real egui
`ArrowDown` event through current-directory selection; toast tests include the rendered-height case that exposed a
two-pixel gap. These focused tests do not substitute for the remaining full keyboard audit.

The 2026-09-13 `.tmp/gallery/file-actions-final/` captures verify square context menus, directory ZIP/folder/removal
actions, compact inspectors, and independent popup state per theme. Narrow captures preserve virtualized listing and
scrollable details while moving side panes into drawers.

Scoped host wiring verification passed 56 device, 43 shared-UI, seven desktop, and 23 HASS tests; native production/demo
Clippy passed with warnings denied, and the actual WASM client compiled. A subsequent memory-capped full QA run recorded
432 workspace tests plus repository, FormatJS, SQLx, license, dependency, gallery, Clippy, and rustdoc checks. These
counts belong to those historical runs only. Manifest-parser cleanup separately recorded 56 device library tests and
warnings-denied checks of affected production/demo clients; its format-dispatch boundary now lives in
[connectivity](../architecture/connectivity.md).

The 2026-09-14 HASS Web demo proof uploaded a test image into a nested activity directory, downloaded it directly, and
downloaded the directory ZIP containing both that image and a FIT file. No CSP violation was reported. Moving the demo
root aside removed the attachment within the polling interval; restoring it recovered the attachment without reload and
preserved the uploaded file. This proves the demo directory transport, not physical-device writes.

The user confirmed that the Edge 1050 and Venu 3S seen together in production were both connected. That observation is
not a full explorer acceptance run on either device. Fresh physical-device proof remains in the device inspection plan;
do not hide OS-visible attachments to compensate for an unverified host/GVFS lifecycle issue.
