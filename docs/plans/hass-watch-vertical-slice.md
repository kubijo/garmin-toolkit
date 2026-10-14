# Home Assistant add-on and hardware acceptance

1. Build self-contained amd64 and aarch64 Home Assistant apps with the managed deployment under persistent `/data` and
   ingress-only mutation access. Configure Supervisor backup as `cold`, then restore a backup containing a non-default
   selected storage generation and verify the application opens that generation. Home Assistant documents both
   [persistent `/data` and cold backup](https://developers.home-assistant.io/docs/apps/configuration/); portable
   application snapshots have a separate [storage contract](../architecture/storage.md#portable-snapshots). The local
   stopped-tree copy test covers the selected-generation layout, but Supervisor integration remains untested.
2. On Raspberry Pi 5, verify ingress-only mutation access, USB permissions and ownership, device disconnects, and the
   complete workflow. Do not assume desktop GIO mounts exist in the app.
3. Complete owned-hardware FIT Course transfer checks: cancellation, interrupted and partial writes, disconnect and
   recovery, readback, and firmware discovery after recovery. The
   [single successful Edge 1050 transfer](../research/usb-sync.md#course-transfer-evidence) establishes only the happy
   path. Verify initial profile pairing and reassociation on physical media separately from Course transfer.

The [shared interface plan](shared-interface-workflows.md) owns client parity and runtime interaction checks; the
[device expansion plan](device-expansion-and-publication.md) owns broader hardware conformance and publication.
