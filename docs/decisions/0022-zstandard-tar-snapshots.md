# 0022: Zstandard tar snapshots

## Decision

Publish `.tar.zst` snapshots: one Zstandard frame containing a POSIX pax archive with exactly `manifest.json`, first,
and `storage.sqlite3`.

Create the database with `VACUUM INTO` under application write control. The manifest records format, schema, app
version, time, length, and SHA-256.

Enable Zstandard checksums, use no dictionary, cap the window, and bound decompression, output, and entry count. Accept
only the two regular files; reject links, devices, duplicates, unexpected names, absolute paths, and traversal.

Before atomic publish or restore, stage and decode; verify structure, digest, length, SQLite integrity, and
compatibility. Restore supports rollback.

Snapshots remain plaintext pending an audited envelope. Exclude credentials, pairing state, caches, logs, maps, rendered
output, and temporary data. This supersedes ADR 0021.

## Why

Zstandard provides bounded streaming, speed, and compression without ZIP's central directory and extension matrix. Tar
adds only the framing needed around SQLite and its manifest.

## Consequences

Snapshots stream through HASS and desktop; restore needs no random access. Test large entries, corruption, truncation,
hostile metadata, limits, versions, interruption, replacement, and rollback.

Source: [RFC 8878](https://www.rfc-editor.org/rfc/rfc8878.html).
