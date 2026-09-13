# Application-wide visual refresh

Apply the original Carbon-derived [visual language](../architecture/visual-language.md) consistently to desktop, HASS
web, and the shared gallery. Nautilus and File Pilot remain observational references only; no screen or component may
copy their assets, palette, dimensions, layout, or distinctive treatment.

## Foundation

- [ ] Express the radius, spacing, control-height, and transient-surface rules as shared primitives rather than repeated
  literals.
- [ ] Review Gray 100 and Gray 10 semantic roles for calm surface separation, readable secondary content, and accessible
  action/focus contrast without replacing the Carbon layering model.
- [x] Give selected content a neutral semantic layer and primary content foreground so selection remains readable in
  both Gray 100 and Gray 10 without borrowing the filled primary-action treatment.
- [x] Keep select triggers, popup surfaces, and option rows square at every control-size variant.
- [x] Keep select popup rows flush with every menu edge and one another; the popup has no inner padding or inter-row
  gap.
- [ ] Modernize shared buttons, icon buttons, fields, selects, menus, dialogs, toasts, progress, and focus indicators.
- [ ] Confirm that no interaction state changes component or row geometry.

## Application shell

- [ ] Modernize the header and primary navigation as one stable shell in expanded, rail, narrow, desktop-window, and web
  variants.
- [x] Clip the complete shell, including foreground profile popups, to the caller-owned application viewport so embedded
  gallery scenes cannot paint over gallery chrome.
- [x] Confirm that rs-gallery's new canvas-containment fix prevents application-shell content from bleeding into gallery
  chrome in Garmin Toolkit's affected scene; verified against the temporary local checkout.
- [x] Keep the shell profile trigger and its attached menu square; retain circular avatars and the established geometry
  of unrelated controls.
- [x] Keep profile-menu rows flush with the menu surface on every edge; the container adds no inner padding or row gap.
- [x] Join the shell header directly to page content without a horizontal divider; keep the navigation pane's right-edge
  divider as the sole internal shell border.
- [ ] Keep only global controls in shell chrome; move page actions into contextual page headers or toolbars.
- [ ] Verify profile chooser, profile menu, device destinations, native window controls, and keyboard focus.

## Pages and workspaces

- [ ] Activities: align search/filter/list/detail hierarchy, empty/loading/error states, and map-ready detail content.
- [ ] Profile entry and settings: replace oversized slabs with bounded groups and modern form/action hierarchy while
  keeping picture selection native to the client.
- [ ] Devices: establish object header, inspection/status summary, capabilities, storage, and contextual actions without
  stretching low-information cards across the viewport.
- [x] Device explorer: apply the same shell, pane, toolbar, row, selection, inspector, and contextual-action language;
  retain its separate functional checklist in [device-explorer-work.md](device-explorer-work.md).
- [ ] Offline, imports, dialogs, notifications, and all failure states: make feedback visually related but semantically
  distinct.
- [x] Keep every toast and severity-rail corner square, and extend each rail over the toast's complete height without
  changing card geometry.

## Evidence

- [ ] Add or update maintained gallery captures for every shared primitive in dark and light themes.
- [ ] Add whole-screen captures for all primary pages at desktop and narrow widths in both themes.
- [ ] Review Czech and long-content scenes for wrapping and geometry stability.
- [ ] Verify focus traversal and keyboard activation on the production components, not gallery stand-ins.
- [ ] Run memory-capped full QA after the complete visual slice passes targeted tests and inspected captures.

## Review findings — 2026-09-13

- [x] Remove the remaining device-summary card radius, including status, capabilities, and storage blocks.
- [x] Limit device-explorer tree side padding to 8–12 pixels and vertically center tree icons and labels inside row
  backgrounds.
- [x] Use a balanced 200-pixel initial tree-pane width. Keep it user-resizable from 160 to 360 pixels so compact and
  long-label catalogs can choose their own working width without making the default feel cramped.
- [x] Let the explorer paint only its actual header and panes rather than an opaque full-stage background. Match the
  rs-gallery checkerboard variant to the light or dark catalog theme wherever the transparent stage remains visible.
- [x] Give the tree title, breadcrumb, and details title one compact 44-pixel header block with identical six-pixel top
  and 10-pixel bottom spacing so all three text baselines align. Prefix the side headings with restrained folder and
  information icons. Keep the middle workspace darker from top to bottom and place only its breadcrumb inside a subtly
  contrasting, input-like field with eight-pixel inner padding. Exercise it at least three directory levels deep in the
  maintained gallery scene.
- [x] Render the explorer path as a subtle field within the middle header: ancestor segments use secondary text, the
  current segment uses primary text, separators are `/`, and every segment remains clickable without changing geometry.
  No segment may resemble a primary submit action.
- [x] Add 12 pixels of inline breathing room to the middle file workspace. Use a lower-alpha subtle color for table
  column rules and add a matching rule below the table header row. Clip the complete table, including row fills,
  separators, header rules, and pointer interaction, to that inset workspace so it cannot bleed into the details pane.
- [x] Move file upload out of the detached page heading and into the current directory context. Present it as a subdued
  tertiary action at the bottom-trailing edge of the middle pane only while the browser has a valid directory target,
  reserve enough space that it never covers the last file row, and use an explicit 12-pixel inset on both axes. Paint
  the reserved bottom strip with the workspace surface so its visible and layout edges are the same edge. Keep the
  tertiary outline neutral in both themes so action blue remains reserved for primary calls to action.
- [x] Put file and directory operations in a square, edge-to-edge context menu on both listing rows and the current
  directory background. Give the popup and all resting rows a distinct layer-one surface; expose file download,
  directory download-as-ZIP, new-folder creation, and removal without making the menu blend into the workspace. Use a
  low-opacity 20-pixel blur for soft elevation rather than a short, dark drop shadow.
- [x] Gate explorer removal behind a danger confirmation that ignores Enter, Space, and accessibility-synthesized
  activation. Only an explicit primary-pointer click on the confirmation button may emit the removal action.
- [x] Keep explorer directory, storage, and toolkit icons neutral. Reserve restrained semantic color for FIT activities
  and generic file types so it helps scanning without competing with primary CTAs.
- [x] Keep the selected-entry inspector compact: icon and item name share one aligned summary row, the path remains
  normal secondary text, and one drive-icon metadata line carries storage plus file size when applicable. Do not use
  tiny field labels or a key/value table for information already conveyed by the icon and hierarchy; constrain summary
  rows to their 20-pixel content height and use a two-pixel rhythm instead of inherited control heights plus spacers.
- [x] Give the breadcrumb field two additional pixels of vertical padding on each side without increasing the shared
  pane-header height. Keep the current segment on the primary text role and mute ancestor segments and separators from
  the secondary role so the active directory is unmistakable without introducing a selected-button treatment.
- [x] Separate the file table's 24-pixel column-header height from its 32-pixel data rows. This raises the `Name` and
  `Size` baselines into visual alignment with the first tree row and details content instead of centering them in an
  unnecessarily tall header cell.
- [x] Add square, compact device bookmarks above the full storage tree only for recognized directories present in the
  catalog. Keep one Storage heading at the top, separate the shortcuts from the tree with a compact rule, and use the
  primary icon role for all user-directory shortcuts rather than borrowing status colors. Keep nested-directory
  selection synchronized with its parent bookmark. Do not promote the toolkit namespace to a bookmark: expose it only in
  the full tree and storage-root listing with a neutral toolbox icon and browse-only explorer policy rather than hiding
  recovery state or falsely presenting the storage itself as read-only.
- [x] Place toast dismiss glyphs eight pixels from the top and trailing edges while retaining a 24-pixel interaction
  target, square cards, and full-height severity rails.
- [x] Make primary-navigation selections square and edge-to-edge, with a full-height accent rail on the navigation
  pane's extreme leading edge.
- [x] Remove parent and row radii from activity lists, remove parent padding, and separate adjacent rows with stable
  one-pixel dividers.
- [x] Set the retained named radius tokens for ordinary buttons, cards, panels, drop zones, and transient surfaces to
  zero. Preserve rounding only for intentionally circular containers such as avatars and status circles.
- [x] Vertically center profile-chooser row contents within their backgrounds.
- [x] Dock modal action groups directly to the trailing and bottom edges, keep two pixels between adjacent actions, and
  reduce the shared content-to-footer gap without adding per-dialog overrides.
- [x] Keep validation-error field borders at the same one-pixel width as resting field borders; error state changes
  semantic color and message content, not outline thickness.
- [x] Use subtle neutral layer colors for open controls and selected/highlighted rows. Reserve filled action blue for
  primary CTA surfaces rather than making selection state resemble a primary button.
- [x] Determine whether light/dark gallery stage-caption and checkerboard contrast is caused by Garmin Toolkit's global
  preview styling or by `rs-gallery`. If upstream owns it, retain generic evidence suitable for an upstream report
  rather than adding a consumer-specific workaround. The generic defect has been reported upstream; do not duplicate
  that report here.
- [x] Restore the gallery and gallery-build dependencies from the temporary local `/home/kubijo/dev/kubijo/rs-gallery`
  checkout to released tag `v0.11.0` after the tested upstream fixes landed.

### Verification record

- The 2026-09-13 visual-polish capture set includes the small, medium, and large select triggers plus a deliberately
  open select. The reviewed images show square trigger, popup, selected-row, and unselected-row geometry, with popup
  rows meeting every menu edge and one another without padding.
- The refreshed 2026-09-13 set under `.tmp/gallery/visual-polish/` was inspected at native resolution. The device card,
  buttons, import drop zone, transient surfaces, navigation selections, activity list, and profile rows are square;
  circular avatars remain circular. Profile rows and explorer tree/table rows are vertically centered, the tree uses
  eight-pixel outer padding, and toast close glyphs sit eight pixels from the top and trailing edges.
- Dark and light explorer captures show stroke-free panes and the darker middle workspace retained from top to bottom.
  Its three compact 44-pixel headers share aligned eight-pixel top and 12-pixel bottom spacing; the side headings carry
  restrained semantic icons and only the breadcrumb is enclosed by a subtly contrasting path field. The explorer no
  longer paints a blanket application surface behind its panes: transparent interstitial space deliberately reveals
  rs-gallery's dark or light checkerboard variant for the selected theme. The file workspace has a small inline inset
  plus low-contrast column and header-bottom rules. The upload action is anchored inside the active directory pane, and
  compact back/forward controls precede the breadcrumb without being mistaken for part of its path.
- The refreshed `modal-form.png` capture shows the shared modal footer docked directly to the trailing and bottom edges,
  two pixels between its actions, and a compact content-to-footer gap. The rule is implemented by the modal primitive
  rather than the profile dialog.
- The refreshed `text-input-states.png` capture shows the validation-error outline at the same one-pixel width as the
  resting field; only its semantic color and supporting message differ.
- The refreshed `select-open-menu.png` capture uses separate neutral surfaces for the open trigger, selected option, and
  resting option. Filled action blue is absent from the selection state and remains available for primary CTAs.
- The 2026-09-13 `local-rs-gallery` capture set resolves all three gallery crates from the adjacent checkout. Its dark
  and light explorer images show the stage caption directly on the checkerboard with readable theme-aware contrast and
  no mismatched outer stage band. The same local build was manually verified to fix Garmin Toolkit's application-shell
  content bleed-over case.
- The final gallery manifest and lockfile resolve `gallery`, `gallery-build`, and `gallery-macros` from released tag
  `v0.11.0`; no adjacent-checkout dependency remains.
- The 2026-09-13 `.tmp/gallery/file-actions-final/` capture set adds dark and light explorer context-menu evidence. Both
  themes show a square, padding-free layer-one popup; directory Download as ZIP, New folder, and separated Remove rows;
  colored folder affordances; and an inspector reduced to an icon/name row, secondary path, and one storage/size line.
  The popup explicitly reapplies the invoking pane's palette because egui popup areas otherwise escape a gallery stage's
  locally scoped light theme.
