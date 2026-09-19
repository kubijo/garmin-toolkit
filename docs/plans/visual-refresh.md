# Application-wide visual refresh

Apply the original Carbon-derived [visual language](../architecture/visual-language.md) consistently to desktop, HASS
web, and the shared gallery. Nautilus and File Pilot remain observational references only; no screen or component may
copy their assets, palette, dimensions, layout, or distinctive treatment.

## Foundation

- [ ] Express the radius, spacing, control-height, and transient-surface rules as shared primitives rather than repeated
  literals.
- [ ] Review Gray 100 and Gray 10 semantic roles for calm surface separation, readable secondary content, and accessible
  action/focus contrast without replacing the Carbon layering model.
- [ ] Modernize shared buttons, icon buttons, fields, selects, menus, dialogs, toasts, progress, and focus indicators.
- [ ] Confirm that no interaction state changes component or row geometry.

## Application shell

- [ ] Modernize the header and primary navigation as one stable shell in expanded, rail, narrow, desktop-window, and web
  variants.
- [ ] Keep only global controls in shell chrome; move page actions into contextual page headers or toolbars.
- [ ] Verify profile chooser, profile menu, device destinations, native window controls, and keyboard focus.

## Pages and workspaces

- [ ] Activities: align search/filter/list/detail hierarchy, empty/loading/error states, and map-ready detail content.
- [ ] Profile entry and settings: replace oversized slabs with bounded groups and modern form/action hierarchy while
  keeping picture selection native to the client.
- [ ] Devices: establish object header, inspection/status summary, capabilities, storage, and contextual actions without
  stretching low-information cards across the viewport.
- [ ] Offline, imports, dialogs, notifications, and all failure states: make feedback visually related but semantically
  distinct.

## Evidence

- [ ] Add or update maintained gallery captures for every shared primitive in dark and light themes.
- [ ] Add whole-screen captures for all primary pages at desktop and narrow widths in both themes.
- [ ] Review Czech and long-content scenes for wrapping and geometry stability.
- [ ] Verify focus traversal and keyboard activation on the production components, not gallery stand-ins.
- [ ] Run memory-capped full QA after the complete visual slice passes targeted tests and inspected captures.

Completed visual rules live in the [visual language](../architecture/visual-language.md); historical captures and test
results live in [interface validation](../research/interface-validation.md). Remove this plan after the remaining
cross-theme, interaction, and whole-screen acceptance work is verified.
