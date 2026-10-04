# Device expansion and publication

Extend operation-specific device evidence, then publish only supported artifacts and claims.

## Work

1. Extend [current map evidence](../research/usb-sync.md#map-maintenance-evidence) to full fēnix 8 Solar and Edge 1050
   conformance. Validate Venu 3S, then Edge 850, following the
   [hardware matrix](../research/device-capabilities.md#validation-hardware).
2. Record identity, metadata, I/O, interruption, preservation, normalization, rendering, export, and recovery per
   device/firmware/host tuple. Mark unsupported cells. Compare CLI, desktop, and HASS inspection and refresh on the same
   physical device, including separate volumes, missing attributes, and read-only media. Check disconnects and stale
   attachment results. Complete consented Venu 3S explorer acceptance; do not hide host/GVFS defects.
3. Investigate battery, charging, and transport health before exposing them. Classify portability, reliability, and
   sensitivity; retain no field without adapter and hardware evidence.
4. Run the local Connect IQ probe after HASS HTTPS exists.
5. Choose release channels, signing and attestations for HASS and desktop from one reproducible build. Evaluate GitHub
   Releases/GHCR or mirrors, and reuse applicable [CLI distribution evidence](distribution.md).
6. Audit configuration, fixtures, identities, URLs, images, notices, and provenance.
7. Publish the support matrix and adaptation guide. Mobile requires a separate decision.

Close when every advertised operation has evidence, artifacts pass policy, and the public tree contains no private data.
