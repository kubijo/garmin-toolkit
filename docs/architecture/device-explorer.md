# Device explorer

The mounted-device explorer uses the [shared window host](application-windows.md): a native secondary viewport on
desktop and an independent browser popup in HASS. The shared Rust/egui explorer fills that window; it is not a system
file picker. It stays open when the main app navigates away from the device page. Its identity, navigation, and close
controls belong to the storage, file, and details pane headers respectively. The native host owns attached storage;
browser clients receive opaque device-relative models.

`egui_ltreeview` provides storage/directory navigation and `egui_extras::TableBuilder` virtualizes the current
directory. Garmin UI primitives own icons, breadcrumbs, actions, and details. The bounded catalog contains at most 4,096
entries; its action model is independent of those UI crates. `rfd` selects a client-side upload file only.

Refresh beside Back and Forward reloads the device catalogue in both shells. It preserves the current folder and any
still-valid selection, falling back to the nearest surviving folder when necessary. Refresh is disabled during file
operations and catalogue requests; it does not repeat an interrupted write.

At narrow widths the subordinate panes become Storage and Details drawers without compressing the listing. Bookmarks
include only recognized directories present in the catalog; the toolkit namespace remains visible but browse-only. Tree,
table, toolbar, and context-menu interactions dispatch the same typed actions. Popups inherit the invoking pane's
palette, including locally scoped gallery themes. The whole window and foreground popups stay inside the caller-owned
viewport. The in-canvas `show_window` wrapper remains available for component previews; application hosts use `show`
inside their platform window.

One bounded host operation layer handles file download, directory ZIP, upload, folder creation, and recursive removal.
FIT Open uses that checked boundary for the shared activity preview; Import uses the duplicate-aware importer. Removal
requires an explicit primary-pointer confirmation; Enter, Space, and accessibility-synthesized activation do not confirm
it. The [shared workflow plan](../plans/shared-interface-workflows.md) retains the broader keyboard audit.

Browser downloads stage bounded results behind random one-use tickets lasting at most 60 seconds. A relative
`<a download>` retrieves an attachment-only, `no-store`, MIME-sniffing-disabled response; file bytes do not travel
through WASM or a generic save dialog. Demo HASS uses an isolated mutable directory transport through the same
operations, including toolkit-namespace protection, refreshed catalogs, and restart/symlink containment.

HASS invalidates pending file operations when either service or parent-window connectivity changes. It does not retry
them automatically. An interruption notice survives reconnect and catalogue refresh, explaining that completion was not
confirmed and directing the user to refresh and check device contents before retrying. Errors received just before
disconnect also remain visible; a lost reply does not establish whether a write committed.

## Composition rationale

The original comparison selected composable tree/table primitives over `egui-file-dialog` 0.15 and `egui_file` 0.28:
their picker lifecycle and local-filesystem UI would require invasive adaptation to opaque device-relative RPC.
`egui-table-kit` 0.6 added unnecessary state for the bounded catalog; complete native file managers were observational
references only. Versions describe that comparison, not an instruction to pin old dependencies.

Maintained evidence and remaining hardware limits are in [interface validation](../research/interface-validation.md).
