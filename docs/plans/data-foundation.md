# Portable data foundation

Complete export contracts against publishable fixtures. Portable snapshot contracts and interoperability coverage live
in [storage architecture](../architecture/storage.md#portable-snapshots).

## Work

Implement a Frictionless Data Package with schema-typed CSV relations, GPX tracks, provenance, and every associated
original FIT artifact for a complete activity. Its descriptor records resources, relations, schemas, and provenance. FIT
remains authoritative. Standalone GPX track sharing is separate; exports are not snapshots. Validate schemas, units,
nulls, IDs, keys, order, and discontinuities against publishable fixtures.

Keep users independent of host and connector identities; keep database handles behind storage; pass actor context
through services; persist no credentials; keep snapshots opaque and exports plaintext. These are compatibility
constraints from [ADR 0023](../decisions/0023-deferred-security-hardening.md), not security claims.

Delete this plan after export semantics have compatibility tests and lasting contracts live in code and architecture.
