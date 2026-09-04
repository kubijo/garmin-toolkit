# 0021: Portable snapshot container

## Decision

Publish ZIP64 snapshots containing `manifest.json` and `nimrag.sqlite3`, created by `VACUUM INTO` under application
write control. The manifest records format, schema, app version, time, length, and SHA-256.

Before atomic publish or restore, stage and reopen the archive; validate its structure, manifest, digest, length,
integrity, and compatibility. Restore supports rollback.

Use conforming JSON/ZIP libraries. Reject duplicate, unexpected, absolute, traversing, or unsupported entries.

Snapshots are plaintext pending an audited envelope. Exclude credentials, pairing state, caches, logs, maps, rendered
output, and temporary data.

## Why

SQLite contains durable data and originals. ZIP is portable; its manifest versions and verifies the payload without
misusing export formats as backup.

## Consequences

Snapshots and exports remain separate. Restore requires connector authentication and device pairing. Test large
archives, corruption, truncation, hostile entries, versions, interruption, replacement, and rollback.
