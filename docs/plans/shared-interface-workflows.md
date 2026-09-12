# Shared interface workflows

Deliver device, FIT, route, activity, asset, and recovery workflows through the same application models and actions in
desktop and HASS. Target shells adapt transport and presentation mechanics without creating parallel behavior.

[The HASS watch slice](hass-watch-vertical-slice.md) owns hosting, ingress, packaging, and hardware deployment. This
plan owns user-visible workflow parity between its browser client and the native desktop app.

## Shared work

1. Populate each device page through automatic bounded inspection, then give it explicit FIT import, route upload, map
   management, and asset-management entry points. File transfer and mutation require separate confirmation.
2. Stage FIT ingress before persistence. Pre-parse selected or dropped files and present their activities, metadata,
   duplicates, warnings, and failures for review. Import nothing until the user explicitly confirms the staged set.
3. Confine drag and drop to visible, enabled targets. Show clear accept or reject feedback while hovering. A drop target
   cannot remain active elsewhere in the window or application.
4. Add a consented mounted-device browser behind an owned `garmin-ui` model. Render storages separately and treat
   optional volume icons as bounded untrusted input.
5. Replace the activity-detail spike with maps, laps, charts, measurements, device data, and provenance.
6. Report mutations through the notification host. Success follows persistence; failures remain visible and actionable
   across reconnects.
7. Prove offline sync, visualization, export, snapshot and restore, and one confirmed upload through both clients.
8. Use only original or individually licensed artwork with generated attribution.
9. Complete a keyboard-only audit of both shells, including focus visibility, traversal order, modal trapping, and
   reconnect recovery.
10. Add redacted structured diagnostics with configurable startup verbosity, bounded live history, rotated log files,
    shared filtering, and explicit download from the UI.

## Target edges

- Desktop composes application services in process. Native pickers remain visibly modal, and system attachment
  differences stay behind device adapters. Test Linux GIO first; test macOS and Windows before advertising them.
- HASS sends owned inputs and opaque staged or operation IDs across the typed service boundary. Browser code receives no
  host paths, storage handles, credentials, or mutation adapters. Reconnect must recover staged review and operation
  state without replaying a write.

Every interface state requires validated `garmin-ui` gallery evidence, including narrow layouts, translated prose,
rejection, partial parsing, confirmation, progress, disconnect, and recovery. HASS loader and ingress behavior also
require captured browser evidence because shared component scenes cannot prove the deployed boundary.

The OS session or HASS ingress grants process access but never identifies a toolkit user. Persist no credentials; keep
snapshots opaque and exports plaintext.

Delete this plan after desktop and HASS complete the same watch workflow through shared contracts, snapshots round-trip
between targets, advertised packages have explicit test tiers, and platform conditions remain inside adapters.
