# 0012: Manifest-driven USB capabilities

## Decision

After consent, `nr-usb` parses `GARMIN/GarminDevice.xml` into transport-neutral capabilities. Apps use them instead of
model tables or MTP paths.

Capabilities expose a supported domain, direction, file shape, and opaque handle bound internally to a validated
relative location. MTP, WPD, and mass-storage adapters share a domain-agnostic file interface. Re-read the manifest on
attachment to observe firmware changes.

Treat manifests as untrusted: reject unsafe paths, malformed/unsupported structures, ambiguous duplicates, and locations
outside the device root. Allowlist domains; ignore firmware, maps, wallet, Wi-Fi, debug, and Connect IQ internals.

Transfer direction describes device layout, not user authorization:

- Output directions permit copying only.
- Input directions identify intake locations; writes also require a supported operation, validated artifact, and user
  action.
- No manifest entry permits deletion of device-produced data.

Classify copied FIT files by `FileId`, not path. Without a valid manifest, allow only user-selected read-only import; do
not guess paths or write.

## Why

Garmin publishes the v2 schema, and the attached fēnix supplies device-specific locations. This outperforms a global
table inferred from one watch. Because the manifest also exposes sensitive firmware surfaces, consent and allowlisting
remain mandatory.

## Consequences

`nr-usb` owns attachment, consented manifest retrieval, validation, capabilities, and file I/O. Domain adapters own FIT
semantics. HASS and desktop share both layers.

Test MTP/mass-storage layouts, unknown types, hostile paths, duplicates, schema changes, and directions. A manifest
proves discovery, not device safety.

See [USB synchronization research](../research/usb-sync.md).
