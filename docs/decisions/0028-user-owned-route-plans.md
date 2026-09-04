# 0028: User-owned route plans

## Decision

A `RoutePlan` is a user-owned aggregate with a stable UUID and immutable revisions, not an observation. Revisions hold
sport, exact geometry or unresolved controls, optional elevation, cues, and provenance.

GPX, freehand drawing, and routing produce the same revision type. Original GPX uses generic artifact storage, never
activity tables. Generated FIT Course files are deployment artifacts.

Each non-empty GPX track segment is a candidate; never concatenate. Routes become unresolved controls and need routing
or confirmed straight lines before deployment. Metadata may suggest but not choose name or sport. Preserve unsupported
content only in the original; never infer cues from waypoints or names.

Reverse and trim create revisions; split creates linked plans. Geographic simplification needs preview and confirmation.
Never join, snap, repair elevation, or alter geometry automatically.

Deployments record revision, FIT bytes, target, and capability evidence under a unique filename without overwrite.
Readback proves transfer; rediscovery or user confirmation separately proves acceptance. Never delete automatically;
cleanup requires an exact project-created partial upload and confirmation.

## Why

Activities are immutable source assertions; routes are editable work. Treating both as observations would entangle
editing, reconciliation, and deployment history.

## Consequences

GPX, routing, FIT, and devices remain separate adapters around shared route services. FIT Course terms stay at device
and artifact boundaries.

See [GPX route-plan research](../research/gpx-route-plans.md).
