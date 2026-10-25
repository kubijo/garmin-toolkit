# Device expansion and publication

## Hardware and other distribution

1. Extend [map evidence](../research/usb-sync.md#map-maintenance-evidence) to full fēnix 8 Solar and Edge 1050
   conformance, then validate Venu 3S and Edge 850 against the
   [hardware matrix](../research/device-capabilities.md#validation-hardware).
2. Test identity, metadata, I/O, interruption, preservation, normalization, rendering, export, and recovery per
   device/firmware/host tuple; mark unsupported cells. Compare CLI/desktop/HASS inspection and refresh, including
   separate volumes, missing attributes, read-only media, disconnects, and stale attachments. Complete consented Venu 3S
   explorer acceptance, retaining host/GVFS defect evidence.
3. Assess battery, charging, and transport telemetry for portability, reliability, and sensitivity before exposing
   fields supported by adapter and hardware evidence.
4. Run the local Connect IQ probe after HASS HTTPS exists.
5. Choose desktop channels, signing, and attestations using [CLI evidence](distribution.md). Extend
   [packaging checks](../research/desktop-packaging.md) to other non-Nix hosts and aarch64. Verify user namespaces, file
   dialogs, production USB/GVfs, and launcher metadata. With the main window covered, test Developer tools cursor
   changes, selection, scrolling, clicks, folder navigation, and close/reopen. Keep driver integration independent of
   the build host; track the AMD-to-NVIDIA failure reproduced in `vkcube` separately.
6. Audit configuration, fixtures, identities, URLs, images, notices, and provenance.
7. Publish the support matrix and adaptation guide. Mobile requires a separate decision.
