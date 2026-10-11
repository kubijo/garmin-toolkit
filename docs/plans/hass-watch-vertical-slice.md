# Home Assistant add-on and hardware acceptance

1. Complete owned-hardware FIT Course transfer checks: cancellation, interrupted and partial writes, disconnect and
   recovery, readback, and firmware discovery after recovery. The
   [single successful Edge 1050 transfer](../research/usb-sync.md#course-transfer-evidence) establishes only the happy
   path. Verify initial profile pairing and reassociation on physical media separately from Course transfer.
2. Integrate Home Assistant host backups with the managed deployment and verify recovery of the selected storage
   generation. Portable application snapshots have a separate
   [storage contract](../architecture/storage.md#portable-snapshots).
3. Build self-contained amd64 and aarch64 add-ons with persistent `/data`. On Raspberry Pi 5, verify ingress-only
   mutation access, USB permissions and ownership, device disconnects, and the complete workflow. Do not assume desktop
   GIO mounts exist in the add-on.
4. Exercise the browser loader with slow, cached, unknown-size, failed, and stale-bundle responses under nested ingress.
   Verify entrypoint and favicon refresh across redeploys, correct precompression negotiation and caching, and resolve
   preload warnings. Measure live serving and save-to-visible latency. The semantic UI client starts after WASM loads,
   so add host-side response-fault controls and startup capture before relying on it for loader acceptance.
5. Verify `just hass::run` exits cleanly on SIGINT and SIGTERM without Just reporting an interrupted recipe.

The [shared interface plan](shared-interface-workflows.md) owns client parity and runtime interaction checks; the
[device expansion plan](device-expansion-and-publication.md) owns broader hardware conformance and publication.
