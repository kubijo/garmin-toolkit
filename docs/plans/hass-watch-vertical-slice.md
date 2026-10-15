# Home Assistant add-on and hardware acceptance

1. Verify the native ARM64 CI image build, then install that image on ARM64 HAOS and exercise its ingress and cold
   Supervisor backup/restore. The
   [AMD64 populated HAOS check](../../infra/integration/hass/README.md#populated-haos-workflow) already covers
   authenticated ingress, persisted GPX/FIT downloads, app restart, and cold backup/restore.
2. On Raspberry Pi 5, verify USB permissions and ownership, device disconnects, and the complete workflow. Do not assume
   desktop GIO mounts exist in the app.
3. Complete owned-hardware FIT Course transfer checks: cancellation, interrupted and partial writes, disconnect and
   recovery, readback, and firmware discovery after recovery. The
   [single successful Edge 1050 transfer](../research/usb-sync.md#course-transfer-evidence) establishes only the happy
   path. Verify initial profile pairing and reassociation on physical media separately from Course transfer.

The [shared interface plan](shared-interface-workflows.md) owns client parity and runtime interaction checks; the
[device expansion plan](device-expansion-and-publication.md) owns broader hardware conformance and publication.
