# 0018: On-device profile marker

## Decision

Persist user-device association in a versioned root TOML marker. First sync writes the selected user UUID, a random
device UUID, and an informational profile snapshot; later attachments read it.

The marker is neither secret nor authentication. UUIDs are immutable; display name and profile revision are refreshed
context. Unknown users and missing or invalid markers require pairing. Garmin and transport IDs remain diagnostics.

Use `toml_edit` to preserve comments, order, formatting, and unknown fields. Never regenerate text. Write, read, parse,
and verify. Replace content only after proving safe device replacement; otherwise retain stale context.

Start with `.nimrag.toml`; test `NIMRAG.TOML` only if owned hardware rejects dotfiles. This supersedes ADR 0017.

## Why

The marker makes offline pairing portable across hosts and installations. Separate user and device UUIDs distinguish
several devices owned by one profile.

## Consequences

Creation requires confirmed device write. Deletion only forgets association. A database cache cannot recreate or
override the marker. Test preservation, malformed/conflicting markers, reconnect, reassociation, and interruption.
