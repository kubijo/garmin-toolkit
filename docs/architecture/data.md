# Data architecture

Garmin Toolkit separates source evidence from derived meaning. Byte-identical artifacts share one content-addressed
BLOB, while artifact and acquisition identities preserve provenance. Normalization records its parser, schema,
transformations, and outcome. Failure retains the artifact but creates no observation.

`garmin-importer` atomically writes provenance, normalization, observations, and projections from complete FIT bytes.
UUIDv5 IDs derive from operation ID and sequence position: exact retries are no-ops; changed reuse conflicts. Parser
rejection retains bytes and failure provenance without projections.

`garmin-services` is the user-scoped application boundary. It returns domain types, never storage rows. Desktop uses it
instead of SQLite; HASS map, activity, and route workflows must enter through the same boundary.

`garmin-fit` classifies every sequence before returning data. Activity sequences retain their source positions; settings
are preservation-only. Malformed or unknown sequences reject normalization, not the source artifact. Activities carry
unit-bearing summaries, laps, ordered points, and timer transitions. Coordinates and measurements are independently
optional. Times have millisecond resolution; summary ends use start plus elapsed. Creator data is diagnostic, and FIT
firmware is a hundredths-scaled number, not semantic versioning.

Observations never merge. Equal versioned fingerprints associate distinct artifacts while retaining every member;
similar data requires user action. Activity fingerprints cover ordered semantics, ignore creator and provenance, and
normalize negative zero. Association never crosses users. Golden vectors lock each fingerprint schema.

Application UUIDs are authoritative; accounts, host users, devices, and FIT serials are connector or diagnostic data.

Profiles carry display and presentation preferences. Avatar import detects bounded PNG, JPEG, or WebP, corrects
orientation, retains the original, and derives an immutable 256-pixel PNG thumbnail. Atomic replacement retains prior
artifacts. Presentation never affects identity or ownership.

Route plans are user-owned editable aggregates, not observations. Immutable revisions hold exact geometry or unresolved
control points, optional elevation, cues, and transformation provenance.

`garmin-gpx` exposes each non-empty track segment as exact geometry and each route as unresolved control points. Empty
elements and standalone waypoints produce no plan; invalid candidates do not hide valid siblings. Import retains the
source and atomically creates only the selected, named candidate. Candidate location stabilizes IDs without merging
selections from one file.

Exact route revisions encode as FIT Course artifacts. Geodesic lengths provide cumulative distance; ordered one-second
timestamps attach cues without predicting travel time. Encoding rejects unresolved or unrepresentable data, and semantic
decoding verifies output behind the parser boundary.

`garmin-route` records a confirmed straight-line interpretation as a new immutable revision. The application advances
the plan head atomically; stale edits fail without partial writes.

See [relational reconciliation](../decisions/0019-relational-reconciliation.md),
[portable identities](../decisions/0015-portable-application-identities.md), and
[route plans](../decisions/0028-user-owned-route-plans.md), and
[backend-neutral color and icons](../decisions/0030-backend-neutral-color-and-icons.md).
