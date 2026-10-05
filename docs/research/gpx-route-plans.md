# GPX route plans

## Existing tools

- Depend on [`gpx` 0.10.0](https://docs.rs/gpx/0.10.0/gpx/) for GPX 1.0/1.1 parsing and writing. It is MIT licensed and
  represents tracks, segments, routes, waypoints, metadata, elevation, and timestamps. Keep its types private.

Preserve original GPX bytes because parsing does not retain every extension. Synthetic fixtures cover both GPX versions,
tracks, segments, routes, optional fields, malformed XML, and invalid geometry.

## Planned-route exports

As of 2026-10-04, [Mapy.com's route-planning help](https://help.mapy.com/route-planning/tools/) documents GPX export.
Cycling, walking, and skiing plans export a full track; car plans can export both a route of control points and a track.
Treat Mapy.com as one producer of ordinary GPX, without account or API integration. Import the full track as exact
geometry. A GPX route containing only control points is unresolved and must not be silently converted to a straight-line
Course. The existing parser separates tracks, segments, and routes; validate the selected candidate before transfer.

Routing-engine selection is deferred with the in-app planner. See
[ADR 0028](../decisions/0028-user-owned-route-plans.md) and the
[HASS watch plan](../plans/hass-watch-vertical-slice.md#remaining-work).
