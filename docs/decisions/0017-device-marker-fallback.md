# 0017: Device marker fallback

## Decision

Use Garmin's manifest Unit ID as the primary physical-device identifier. If a consented USB device exposes no stable
identifier, offer to create a versioned root marker containing only a random Nimrag marker ID. Keep user associations in
the database.

Never overwrite a marker automatically. Validate an existing marker; prompt on malformed or conflicting content. Stage,
write, read back, and verify creation. Marker absence after reset or replacement starts pairing again.

This is the only exception to ADR 0012's no-write fallback when a manifest cannot establish identity. It still requires
explicit consent, a writable root, collision checks, and verification; failure grants no other write capability.

## Why

The marker provides deterministic reconnect UX without fingerprints or transport assumptions. It is not authentication;
physical access and explicit consent are already required.

## Consequences

USB/MTP/FIT serials remain corroborating aliases. VID/PID, model, storage ID, path, and port never identify a unit.
Marker writing uses the same gated device-write boundary as other mutations and stores no profile name, user ID, or
secret.
