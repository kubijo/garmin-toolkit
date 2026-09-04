# GPX route plans

## Existing tools

- Depend on [`gpx` 0.10.0](https://docs.rs/gpx/0.10.0/gpx/) for GPX 1.0/1.1 parsing and writing. It is MIT licensed and
  represents tracks, segments, routes, waypoints, metadata, elevation, and timestamps. Keep its types private.
- Depend on [`rapidgeo-simplify` 0.1.2](https://docs.rs/rapidgeo-simplify/0.1.2/rapidgeo_simplify/) for confirmed
  Douglas–Peucker simplification with great-circle meter tolerance. It is MIT OR Apache-2.0 licensed.
- Do not apply `geo::Simplify` directly to longitude/latitude: its tolerance uses planar coordinate units.

Preserve original GPX bytes because parsing does not retain every extension. Synthetic fixtures cover both GPX versions,
tracks, segments, routes, optional fields, malformed XML, and invalid geometry.

## Routing

The engine remains open. [BRouter](https://github.com/abrensch/brouter) and
[GraphHopper](https://github.com/graphhopper/graphhopper) provide mature cycling and foot profiles but require a JVM;
current Rust candidates such as [routx](https://github.com/MKuranowski/routx) and
[Itinera](https://geolang.github.io/itinera/) lack equivalent evidenced cycling and running policy. Keep one owned
interface for basic cycling and running requests, control points, geometry, cues, elevation, errors, and engine/data
provenance.

See [ADR 0028](../decisions/0028-user-owned-route-plans.md) and
[OQ-022](../plans/open-questions.md#oq-022-route-plan-routing).
