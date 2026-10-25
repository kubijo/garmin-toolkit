# Home Assistant add-on and hardware acceptance

The [release pipeline](../../infra/release/README.md) owns artifacts, provenance, the installable app repository, and
registry-based acceptance. [Publication readiness](../research/publication-readiness.md) records HAOS guest evidence.
This plan owns physical-device and platform qualification.

1. On Raspberry Pi 5, verify USB permissions and ownership, device disconnects, and the complete workflow. Do not assume
   desktop GIO mounts exist in the app.
2. Complete owned-hardware FIT Course transfer checks: cancellation, interrupted and partial writes, disconnect and
   recovery, readback, and firmware discovery after recovery. The
   [single successful Edge 1050 transfer](../research/usb-sync.md#course-transfer-evidence) establishes only the happy
   path. Verify initial profile pairing and reassociation on physical media separately from Course transfer.

The [shared interface plan](shared-interface-workflows.md) owns client parity and runtime interaction checks; the
[device expansion plan](device-expansion-and-publication.md) owns broader hardware conformance and publication.
