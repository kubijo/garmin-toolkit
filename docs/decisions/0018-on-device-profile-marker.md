# 0018: On-device profile marker

## Decision

Persist user-device association in `GARMIN-TOOLKIT/pairing.toml`. The first confirmed pairing writes the selected user
UUID, a random device UUID, an informational profile name, and revision zero. Later attachments inspect this document
and any create-only `pairing-000001.toml`, `pairing-000002.toml`, … revisions in the same directory. The format uses the
same `garmin-toolkit-device-state` magic, `kind = "pairing"`, and version header as the other state files.

The marker is neither secret nor authentication. The device UUID stays fixed; a reassignment records a new user UUID and
profile-name snapshot in the next revision. A valid marker can identify a known profile, but it does not bypass profile
selection or grant access. Unknown users, missing markers, and invalid revision chains need explicit review. Garmin and
transport IDs remain diagnostics.

Use `toml_edit` to preserve comments, order, formatting, and unknown fields. Never regenerate text. Write, read, parse,
and verify. Reassignment requires separate consent and creates a new revision without replacing any prior file. Require
a contiguous history of at most 64 revisions with matching device UUIDs; a malformed, missing, or conflicting revision
fails closed.

Do not create a separate marker at the device root.

## Why

The marker makes offline pairing portable across hosts and installations. Separate user and device UUIDs distinguish
several devices owned by one profile.

## Consequences

Creation requires confirmed device write. A database cache cannot recreate or override the pairing documents. Test
preservation, malformed/conflicting documents, reconnect, reassociation, and interrupted writes. Generic device-browser
file actions do not mutate this namespace.
