# 0028: User-owned route plans

## Decision

A `RoutePlan` is a user-owned aggregate with a stable UUID and immutable revisions, not an observation. Revisions hold
sport, exact geometry or unresolved controls, optional elevation, cues, and provenance. The current user workflow
imports GPX planned elsewhere, previews a selected candidate, and converts exact track geometry to a FIT Course. It does
not include an in-app route planner or routing engine.

Original GPX uses generic artifact storage, never activity tables. Each generated FIT Course is a separate, immutable
artifact with its own identity and digest, linked to the exact route revision and encoder version. A Course can be
downloaded or sent to a device without changing the route revision. Regeneration creates a new artifact.

Each non-empty GPX track segment is a candidate; never concatenate. Routes become unresolved controls and need routing
or confirmed straight lines before deployment. Metadata may suggest but not choose name or sport. Preserve unsupported
content only in the original; never infer cues from waypoints or names.

Any future geometry change must create a revision rather than overwrite the imported route. Do not join, snap, repair
elevation, or alter geometry automatically. Freehand drawing, routing, reverse, trim, split, and simplification are
outside the current workflow.

Deployments record the Course artifact, revision, target, and capability evidence under a unique filename without
overwrite. Readback proves transfer; rediscovery or user confirmation separately proves acceptance. Never delete
automatically; cleanup requires an exact project-created partial upload and confirmation.

## Why

Activities are immutable source assertions; imported routes are user-selected plans. Treating both as observations would
entangle activity reconciliation with route provenance and deployment history.

## Consequences

GPX, FIT, and devices remain separate adapters around shared route services. FIT Course terms stay at device and
artifact boundaries. GPX import is source-neutral; one route-planning website is not an application dependency.

See [GPX route-plan research](../research/gpx-route-plans.md).
