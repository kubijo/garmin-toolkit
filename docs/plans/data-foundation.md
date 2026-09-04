# Portable data foundation

Complete snapshot and export contracts against publishable fixtures.

## Work

1. Implement [verified `.tar.zst` snapshots](../decisions/0022-zstandard-tar-snapshots.md) with staged restore.
2. Implement the [open export package](../decisions/0020-composed-open-exports.md) and standalone GPX sharing.
3. Test corruption, truncation, duplicates, unknown fields, interruption, and incompatible restore.

Keep users independent of host and connector identities; keep database handles behind storage; pass actor context
through services; persist no credentials; keep snapshots opaque and exports plaintext. These are compatibility
constraints from [ADR 0023](../decisions/0023-deferred-security-hardening.md), not security claims.

Delete this plan after clean-root restore is atomic, export semantics have compatibility tests, and lasting contracts
live in code and architecture.
