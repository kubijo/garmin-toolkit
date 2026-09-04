# 0014: Single SQLite storage

## Decision

Use one SQLite database per deployment through SQLx 0.9.0, checked SQL, immutable migrations, WAL, foreign keys, a busy
timeout, and full synchronous durability. Application services enforce users, ownership, and sharing.

Store content-addressed originals and queryable projections together. Retain provenance and parser/schema versions for
deterministic reprocessing; originals are authoritative and projections replaceable.

Retain a complete malformed artifact and failed normalization without observations. Never promote partial reads. Exact
acquisition retries are idempotent; later syncs may add provenance for the shared BLOB.

Create snapshots with staged `VACUUM INTO`, verification, and atomic publication. Exclude secrets, maps, rendered
output, logs, and caches.

## Why

One database makes import, derivation, sharing, migration, and snapshots transactional. Per-user databases complicate
sharing; external artifacts add a consistency boundary; PostgreSQL requires a service. SQLx provides checked queries and
migrations without an ORM model.

## Consequences

Keep the database local and never copy it live; browser code has no direct access. Export and deletion are explicit.
Deletion previews impact, requires confirmation, commits atomically, and collects only unreferenced artifacts. A
separate content tier requires evidence that source artifacts are impractical in SQLite.
