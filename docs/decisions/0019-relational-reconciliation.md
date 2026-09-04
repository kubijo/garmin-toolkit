# 0019: Relational reconciliation without merging

## Decision

Never merge artifacts or observations. Byte-identical artifacts share a deployment-wide content-addressed BLOB, while
acquisitions and observations retain user-scoped identities and provenance.

Versioned typed projections produce canonical fingerprints per observation. Equal fingerprints associate without
selecting or combining; unequal similarity requires user action. Automatic association never crosses users. Conflicts
retain every value and source without precedence.

Store artifacts, provenance, normalization, observations, groups, and normalized domain data relationally, not as nested
JSON.

Unknown fields remain in the artifact and enter fingerprints only when understood. Schemas define types, units, missing
values, order, granularity, and version.

## Why

Raw hashes include encoding noise. Semantic fingerprints recognize known equivalence while immutable artifacts preserve
unknown data and evidence. Groups express equivalence without consolidation.

## Consequences

One artifact may have many acquisitions and observations. Activity groups usually associate whole-file projections;
wellness data may associate individual measurements. Derived views expose members and provenance. Reprocessing creates
new versioned projections without changing artifacts.
