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

- [ ] Activities: implement the [calendar header](#activity-calendar-header), including empty/loading/error states.
- [ ] Profile entry and settings: replace oversized slabs with bounded groups and modern form/action hierarchy while
  keeping picture selection native to the client.
- [ ] Devices: establish object header, inspection/status summary, capabilities, storage, and contextual actions without
  stretching low-information cards across the viewport.
- [ ] Offline, imports, dialogs, notifications, and all failure states: make feedback visually related but semantically
  distinct.

### Activity calendar header

Agreed layout; implementation pending for desktop and HASS.

- Replace the permanent activity-list and right-hand details columns with one header above a full-width map. Keep the
  application sidebar. Remove redundant outer page padding while retaining padding around text and controls.
- Show an **expanded, always-visible month calendar** in the header, beside the selected activity's title, date/time,
  distance, duration, and ascent. Mark days containing activities and highlight the selected day.
- Calendar arrows change month. Separate previous/next activity controls step chronologically through activities,
  including those on the same day, skipping empty days and updating the displayed month when needed.
- Selecting a new day opens its first activity; selecting the current day preserves the selected activity. Show a
  selectable list beside the calendar **only when that day contains multiple activities**, with time, sport/name, and
  distance. Otherwise show just the selected activity's heading and summary.
- Allow space for six calendar week rows; avoid another title or toolbar above this header. Keep charts and laps below
  the map, with contextual inspection when needed instead of a permanent details column.
- Retain an **All activities** action for searching and filtering the archive.

Before implementation, settle empty-day selection, date/time-zone grouping, and narrow-screen arrangement. Verify
month/year boundaries, multiple activities per day, navigation endpoints, keyboard access, and both themes.

## Evidence

- [ ] Add or update maintained gallery captures for every shared primitive in dark and light themes.
- [ ] Add whole-screen captures for all primary pages at desktop and narrow widths in both themes.
- [ ] Review Czech and long-content scenes for wrapping and geometry stability.
- [ ] Verify focus traversal and keyboard activation on the production components, not gallery stand-ins.
- [ ] Run memory-capped full QA after the complete visual slice passes targeted tests and inspected captures.

Completed visual rules live in the [visual language](../architecture/visual-language.md); historical captures and test
results live in [interface validation](../research/interface-validation.md). Remove this plan after the remaining
cross-theme, interaction, and whole-screen acceptance work is verified.
