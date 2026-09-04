# Device expansion and publication

Extend operation-specific device evidence, then publish only supported artifacts and claims.

## Work

1. Extend [current map evidence](../research/usb-sync.md#map-maintenance-evidence) to full fēnix 8 Solar and Edge 1050
   conformance. Follow [the owned-device order](../decisions/0007-owned-device-validation-order.md) for Venu 3S, then
   Edge 850.
2. Record identity, metadata, I/O, interruption, preservation, normalization, rendering, export, and recovery per
   device/firmware/host tuple. Mark unsupported cells.
3. Investigate battery, charging, and transport health before exposing them. Classify portability, reliability, and
   sensitivity; retain no field without adapter and hardware evidence.
4. Run the local Connect IQ probe after HASS HTTPS exists.
5. [OQ-016](open-questions.md#oq-016-release-and-publication) owns HASS/desktop release policy. Reuse applicable
   [CLI distribution evidence](distribution.md).
6. Audit configuration, fixtures, identities, URLs, images, notices, and provenance.
7. Publish the support matrix and adaptation guide. Mobile requires a separate decision.

Delete this plan after every advertised operation has recorded evidence, release artifacts pass policy, the public tree
contains no private data, and the repository is understandable without plans.

[Mounted-device updates](mounted-device-updates.md) owns CLI map-upgrade proof on the current devices. This plan owns
broader device conformance and the published support matrix; a successful map operation proves no FIT or HASS workflow.
