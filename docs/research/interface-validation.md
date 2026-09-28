# Interface validation coverage

Render and inspect the maintained [gallery capture sets](../../infra/gallery/captures/) after UI changes:

- `activity-calendar.capture.toml`: Calendar states, regional week starts, Hebrew calendar, archive, narrow/wide
  layouts, English/Czech, both themes.
- `activity-map.capture.toml`: Map, chart statistics, retained cursor, laps, loading/failure, shared analysis surfaces.
- `desktop.capture.toml`: Device states and desktop workspace; equal-width import/axis buttons, English/Czech, narrow
  layouts.
- `visual-language.capture.toml`, `components.capture.toml`: Shared controls, surfaces, menus, notifications and focus
  states.
- `device-explorer.capture.toml`: Integrated/narrow explorer, selection, FIT preview, empty/error states.

The opt-in `capture_calendar_hover` test in [activity_calendar.rs](../../infra/gallery/tests/activity_calendar.rs)
renders selected/unselected hover states in both themes. Calendar interaction tests verify disabled days and cursor
feedback.

[Control API checks](../architecture/developer-tools.md) exercise live applications. Maintained
[scenarios](../../crates/garmin-ui/src/automation/scenarios.rs) cover activity interactions and responsive layouts;
[automation tests](../../crates/garmin-ui/src/automation/tests.rs) cover delayed login readiness and input handling.

Gallery captures establish layout and styling, not live tile-provider readiness or device transport. Simultaneous Edge
1050 and Venu 3S attachment was confirmed; full explorer acceptance remains open. HASS filesystem checks do not
establish physical-device writes.

Remaining work: [visual refresh](../plans/visual-refresh.md),
[shared workflows](../plans/shared-interface-workflows.md), [device inspection](../plans/device-state.md).
