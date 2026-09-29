# Portable data foundation

Complete export contracts against publishable fixtures. Snapshot coverage includes native file save/restore, HASS server
files, and a large native snapshot transferred through HASS and reopened locally.

## Work

Implement the [open export package](../decisions/0020-composed-open-exports.md) and standalone GPX sharing separately.

Keep users independent of host and connector identities; keep database handles behind storage; pass actor context
through services; persist no credentials; keep snapshots opaque and exports plaintext. These are compatibility
constraints from [ADR 0023](../decisions/0023-deferred-security-hardening.md), not security claims.

Delete this plan after clean-root restore is atomic, export semantics have compatibility tests, and lasting contracts
live in code and architecture.
