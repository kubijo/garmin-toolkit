# GPX route plans

## Existing tools

- Depend on [`gpx` 0.10.0](https://docs.rs/gpx/0.10.0/gpx/) for GPX 1.0/1.1 parsing and writing. It is MIT licensed and
  represents tracks, segments, routes, waypoints, metadata, elevation, and timestamps. Keep its types private.

Preserve original GPX bytes because parsing does not retain every extension. Synthetic fixtures cover both GPX versions,
tracks, segments, routes, optional fields, malformed XML, and invalid geometry.

## Native parser containment

The library constructs a whole document without configurable allocation limits. Native upload workflows use the packaged
`garmin-gpx-worker`, with the same GPX library inside a separate process. On Linux it installs a 512 MiB address-space
limit, five seconds of CPU time, and disabled core dumps before reading input. The parent permits one parser at a time,
applies a ten-second wall deadline, caps input at 16 MiB and output at 32 MiB, and kills/reaps failed or timed-out
workers. Other platforms currently reject contained parsing rather than falling back to an unlimited host parse. These
are resource limits, not a filesystem or network sandbox.

The private versioned protocol validates input digest, parser version, candidate uniqueness, geometry, coordinates, and
elevations. It allows at most 128 candidates, 4096-byte suggested names, and one million route points. `PreparedGpx`
retains the original bytes and validated document; `Application::import_prepared_route` checks that input binding and
commits the selected candidate without reparsing. The direct parser/import APIs remain for trusted native callers and
fixtures; upload adapters must use the contained path.

Tests run the real helper, inspect its installed process limits, exercise rejection and timeout cleanup, reject invalid
protocol replies, and confirm a contained preview through persisted import and FIT Course generation. Packaging recipes
include the helper beside both native hosts.

Demo AppImage acceptance on 2026-10-06 passed GPX import/preview, byte-exact GPX/FIT exports, deletion/cancellation,
versioned regeneration, and persistence across restart. Fresh-demo seeding and device transfer were not covered.
Automation verification limits are recorded in [developer tools](../architecture/developer-tools.md).

Packaged HASS VM acceptance on 2026-10-07 passed all ten [route/recovery cases](../../infra/integration/hass/README.md):
seeded recorded activities/routes, reviewed import, rejected siblings and unresolved controls, byte-exact GPX/FIT
exports, versioned deletion/regeneration, profile isolation, narrow layouts, interrupted uploads, lost-response retries,
queued requests across restore, and graceful/abrupt restarts. Assertions compare persisted artifacts and selected
storage, including duplicate-free seeding. An intentional-failure probe also verified process and state cleanup; the VM
shut down after collecting diagnostics.

Earlier headed-browser acceptance additionally checked the seeded walking/hiking activity maps and charts. The VM uses
software rendering without external map tiles. Live restore during active Course encoding or streaming download, and
device transfer, remain unverified.

## Planned-route exports

As of 2026-10-04, [Mapy.com's route-planning help](https://help.mapy.com/route-planning/tools/) documents GPX export.
Cycling, walking, and skiing plans export a full track; car plans can export both a route of control points and a track.
Treat Mapy.com as one producer of ordinary GPX, without account or API integration. Import the full track as exact
geometry. A GPX route containing only control points is unresolved and must not be silently converted to a straight-line
Course. The existing parser separates tracks, segments, and routes; validate the selected candidate before transfer.

Routing-engine selection is deferred with the in-app planner. See
[ADR 0028](../decisions/0028-user-owned-route-plans.md) and the
[HASS watch plan](../plans/hass-watch-vertical-slice.md#remaining-work).
