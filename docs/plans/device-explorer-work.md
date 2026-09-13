# Production device and explorer work

Track the production-device defects and the mounted-device explorer as one vertical slice. Do not treat the explorer as
a native or browser file picker: it is an application-owned, resizable egui window over the active device page, backed
by storage attached to the server or desktop host. Its window controls are incorporated into the explorer's pane
headers: identity in the storage segment, navigation in the file segment, and close in the details segment.

## Production device inspection

The Edge 1050 shown in production is not demo state: Linux reports Garmin USB product `091e:5158` and an active GVFS MTP
mount. Production inventory must continue to show OS-visible attachments. If that device is physically absent, the
remaining work is to diagnose the host/GVFS mount lifecycle rather than hide it in application code.

## Implementation gate

Before implementing the replacement, compare maintained egui-compatible crates and existing workspace primitives for:

- application-owned in-canvas window composition rather than a native or browser picker;
- virtual/remote file-system models;
- list/tree navigation, keyboard focus, selection, double-click, context menus, and drag-and-drop;
- custom icons, file-type actions, detail sidebars, and application-owned toolbars;
- stable layout and theme integration;
- native and WebAssembly support, maintenance, dependency cost, and license compatibility.

Prefer adopting or composing maintained primitives over writing a file manager widget from scratch. Record the selected
option and rejected alternatives here before implementation.

### Options reviewed

| Option              | Verdict                    |
| ------------------- | -------------------------- |
| `file-dialog`       | Remove: window picker      |
| `egui_file`         | Reject: fewer hooks        |
| Full managers       | Reference only             |
| `table-kit`         | Reject: too much machinery |
| `ltreeview` + table | **Select: shared window**  |

- [`egui-file-dialog` 0.15][file-dialog] has useful navigation and virtual-FS behavior, but its public UI owns a picker
  lifecycle and local-file semantics. Adapting it to opaque device-relative RPC would require a maintained fork of
  private UI code.
- [`egui_file` 0.28][egui-file] has the same window/picker mismatch and fewer extension points.
- Full file managers (`fileman`, `rcmd`, `guth`) are interaction references only. Their native application and local-I/O
  models do not fit HASS RPC or WebAssembly.
- [`egui-table-kit` 0.6][table-kit] brings general-table state and dependencies that are excessive for the bounded
  4,096-entry catalog.
- [`egui_ltreeview` 0.9][treeview] and the existing [`TableBuilder`][table-builder] provide composable tree navigation
  and virtualized rows inside the shared egui window. We retain control of icons, actions, breadcrumbs, details, and the
  remote catalog adapter.

Use `egui_ltreeview` for storage and directory navigation and `TableBuilder` for the current directory. Use existing
Garmin UI buttons/icons for the toolbar and details pane. Keep `rfd` only for choosing a PC file to upload. The
catalog/action model stays independent of all three UI crates, so desktop and HASS route the same opaque device-relative
operations.

## Acceptance evidence

- The maintained `device-explorer.capture.toml` matrix renders and was reviewed as an integrated window over the device
  page, as a narrow responsive window, at storage root, a deeply nested folder, selected generic and FIT files, empty
  storage, inspection failure, both themes, and Czech.
- [ ] Production proof covers the real Venu 3S and distinguishes any genuinely OS-visible Edge attachment from demo
  fixtures.

### Verification record

- The maintained `components.capture.toml` set renders the populated explorer surface, expanded mixed-height toasts, and
  a 360-pixel-wide mixed-height toast stack. The captures show one aligned storage/folder affordance per tree row,
  compact explorer navigation, centered table content, and consistent gaps between all toast cards.
- A real egui `ArrowDown` event is covered from focused current-directory selection through the browser state update;
  pure row-boundary movement is covered separately.
- Toast positioning has both arithmetic coverage and a rendered-height regression for the title-only card that exposed
  the previous two-pixel visual gap.
- The populated explorer gallery render verifies 24-pixel directory rows without an ambient inter-row gap and a
  dedicated icon slot that leaves a consistent visual gap before every storage and folder label.
- The narrow explorer capture verifies that the side panes collapse instead of compressing the listing. Storage and
  Details remain available from the path toolbar as overlaid drawers; bookmarks and the directory tree share a scroll
  region, the listing retains its virtualized scroll body, and long detail summaries scroll above fixed compact actions.
- Dark and light context-menu captures under `.tmp/gallery/file-actions-final/` verify the type-aware directory menu,
  distinct floating surface, semantic folder color, and selected-directory inspector without a micro key/value table.
  Its type is implicit in the icon and its remaining location metadata is one ordinary secondary line. The forced-open
  gallery fixtures keep separate state per theme so popup state never crosses isolated capture contexts.
- The 2026-09-13 `visual-language.capture.toml` render adds dark and light explorer evidence plus an always-open shell
  profile-menu scene. The reviewed images show contiguous square explorer panes, aligned 48-pixel pane headers, padded
  and centered table cells, a clipped square profile surface, square full-height toast rails, and no shell header-bottom
  divider. All output lives under `.tmp/gallery/visual-polish/`.
- The 2026-09-14 shell render under `.tmp/gallery/stateful-nav-toggle/` verifies a 32-pixel native title bar, square
  window controls, a proportionate profile avatar, a matching 32-pixel collapsed navigation rail, and stateful
  outdent/indent navigation controls. The maintained capture matrix now covers both Rail and Narrow shell states. All 63
  shared-UI tests and strict all-feature Clippy passed.
- Memory-capped `just qa::full` passed: repository/Python checks, FormatJS, SQLx, workspace Clippy and rustdoc, 432
  workspace tests, license checks, `cargo-deny`, `cargo-machete`, gallery Clippy and rustdoc, two gallery tests, and
  gallery dependency policy.
- The completed host wiring uses one bounded, device-relative operation layer for file download, directory ZIP, upload,
  new-folder creation, and recursive removal. The 2026-09-13 scoped verification passed 56 device tests, 43 shared-UI
  tests, seven desktop tests, and 23 HASS tests; production and demo native Clippy passed with warnings denied, and the
  actual `wasm32-unknown-unknown` HASS client compiled successfully. FIT Open now reads through that same checked
  boundary into the shared activity preview, while Import uses the existing duplicate-aware application importer.
- Browser downloads no longer pass file bytes into WASM or use `rfd`'s generic save fallback. The host stages each
  bounded result behind a random one-use ticket for at most 60 seconds, and the canvas triggers a relative
  `<a download>` whose HTTP response is attachment-only, `no-store`, and MIME-sniffing-disabled.
- Demo HASS now recreates an isolated mutable device tree under its caller-owned data root and runs the production
  browser operations against the directory transport. Service coverage creates a directory, uploads and downloads a
  file, downloads the directory as ZIP, recursively removes it, observes each refreshed catalog, rejects mutation below
  `GARMIN-TOOLKIT`, and verifies restart and stale-symlink containment.
- The 2026-09-14 HASS Web runtime proof uploaded `angry_girl.jpg` into `Garmin/Activity/History/2026`, downloaded it
  directly without an intermediate dialog, and downloaded `2026.zip` containing that image and the existing FIT file.
  Neither the browser console nor the host reported a CSP violation. Moving the demo device root aside removed the
  attached device within the polling interval; restoring it made the device reappear without a page reload, and the
  uploaded image remained present after reconnect.

[egui-file]: https://docs.rs/egui_file/0.28.0/egui_file/
[file-dialog]: https://docs.rs/egui-file-dialog/0.15.0/egui_file_dialog/
[table-builder]: https://docs.rs/egui_extras/0.36.2/egui_extras/struct.TableBuilder.html
[table-kit]: https://github.com/xangelix/egui-table-kit
[treeview]: https://docs.rs/egui_ltreeview/0.9.0/egui_ltreeview/
