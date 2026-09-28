# Visual refresh

Apply the [visual language](../architecture/visual-language.md) across shared components and primary pages.

- Centralize remaining radius, spacing, control-height, and transient-surface literals.
- Review Gray 100/10 roles and shared controls, menus, dialogs, notifications, progress, and focus indicators.
  Interaction states must preserve geometry.
- Review expanded/rail/narrow/native/web shell, global versus page actions, profile navigation, and window controls.
- Complete rebuilt desktop/HASS activity-calendar acceptance: import, narrow scrolling, map/chart/lap interaction. Use
  `infra/gallery/captures/activity-calendar.capture.toml`.
- Compact profile forms; review device headers, capabilities, storage, and contextual actions.
- Review offline/import/error states, Czech and long content, focus traversal, and keyboard activation.
- Inspect dark/light and narrow/full-screen captures for all affected production components; run full QA.

[Validation coverage](../research/interface-validation.md) links maintained visual checks. Delete after remaining
runtime and cross-theme acceptance passes.
