# 0020: Composed open exports

## Decision

Export a Frictionless Data Package: schema-typed CSV for relational data and GPX for tracks and course geometry. Its
descriptor records resources, relations, schemas, and provenance.

Complete activity exports include every associated original FIT artifact and provenance. FIT remains authoritative; open
resources are additive views.

Activities with positions also offer standalone GPX containing only the shareable track.

Versioned tables cover the domain and provenance. Do not adopt KRD, TCX, ZWO, or a project-specific monolith.

## Why

No open fitness format covers FIT's domain. Data Package composes heterogeneous resources, Table Schema describes CSV
relations, and GPX serves track sharing. Each file remains useful alone.

Sources: [Data Package](https://specs.frictionlessdata.io/data-package/),
[Table Schema](https://specs.frictionlessdata.io/table-schema/).

## Consequences

Test schemas, units, nulls, IDs, keys, and provenance. GPX preserves order, optional timestamps, and discontinuities
while documenting omissions. Exports are not snapshots.
