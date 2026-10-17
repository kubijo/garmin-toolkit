# Portable data foundation

Complete export contracts against publishable fixtures. Portable snapshot contracts and interoperability coverage live
in [storage architecture](../architecture/storage.md#portable-snapshots).

## Work

Implement a Frictionless Data Package with schema-typed CSV relations, GPX tracks, provenance, and every associated
original FIT artifact for a complete activity. Its descriptor records resources, relations, schemas, and provenance. FIT
remains authoritative. Standalone GPX track sharing is separate; exports are not snapshots. Validate schemas, units,
nulls, IDs, keys, order, and discontinuities against publishable fixtures.

Add compatibility tests for the export semantics before claiming the format stable.
