# Home Assistant watch vertical slice

Verify the Course transfer and separate device pairing flows in the packaged add-on and on physical media.
[Shared workflows](shared-interface-workflows.md) owns desktop parity; physical inspection evidence belongs to
[device expansion](device-expansion-and-publication.md). Retain the native-host/WASM and
[typed service](../architecture/workspace.md) architecture; Bluetooth and HASS domain publication remain excluded.
Device reassociation has [demo acceptance](../research/usb-sync.md#demo-pairing-acceptance).

## Remaining work

1. Repeat the reviewed Course transfer on owned hardware. Check mounted-MTP disconnect/recovery and firmware acceptance;
   the demo confirms byte readback and explicit user acceptance, not firmware behavior.
2. Integrate Home Assistant host backups with the managed deployment and verify recovery of its selected storage
   generation. Portable application snapshots have a separate
   [storage contract](../architecture/storage.md#portable-snapshots).
3. Build self-contained amd64/aarch64 add-ons with persistent `/data`; verify ingress-only mutation access, USB
   permissions/ownership, disconnects, and workflow on Raspberry Pi 5. Do not assume desktop GIO mounts exist. Verify
   [profile marker](../decisions/0018-on-device-profile-marker.md) initial pairing and reassociation in the add-on and
   on physical media; initial pairing has unit coverage but still needs live acceptance.

The packaged HASS demo has [asserted VM coverage](../../infra/integration/hass/README.md) for reviewed GPX import,
Course versions and downloads, reviewed transfer with exact device bytes and unchanged pairing, restart reconciliation,
restore at the stated boundaries, and duplicate-free demo seeding. Desktop parity and offline/device-free acceptance
remain in [shared workflows](shared-interface-workflows.md). Keep the
[route client contract](../architecture/storage.md#route-client-workflow) and
[ADR 0028](../decisions/0028-user-owned-route-plans.md) as the durable owners.

Keep focused validation for GPX 1.0/1.1, multiple segments, malformed and oversized input, optional elevation, retained
original bytes, and distinct walking/hiking/running/cycling through FIT and snapshot restore. The HASS VM exercises only
the fixture paths named in its README; it does not replace the parser and storage test matrix.

## Device transfer contract

Transfer a selected persisted Course artifact. Reuse device inspection, capability evidence, write exclusion,
cancellation, and recovery from shared device workflows. The one-time approval is scoped to the requesting profile's
artifact digest/version, the selected device identity and storage, and a create-only filename. Confirmation must be
invalidated by a changed target or artifact. Do not expose a generic write-path RPC. Manual transfer does not pair the
selected device or alter an existing pairing marker; pairing only enables automatic profile opening and device sync.

Persist transfer intent before mutation and reconcile interrupted writes using the exact recorded destination and
digest. Readback records transfer verification; firmware rediscovery or explicit user confirmation records acceptance
separately. Cleanup remains a separately approved operation restricted to this transfer's partial upload. Test with the
demo device first; an owned-device trial needs explicit authorization and capability evidence.

### Hardware acceptance still needed

- Exercise cancellation, disconnect, partial write recovery, and readback on owned hardware without unintended mutation.
- Observe firmware discovery separately from verified bytes and record the result.

Use the shared UI for desktop parity; the browser never acquires filesystem or device access. Keep the implementation
source-neutral and add no Mapy.com account/API dependency.

## Browser/process acceptance

- Inspect the actual DOM loader under slow/cached/unknown-size/failed/stale-bundle responses and nested ingress.
- Verify normal reload receives current entrypoint/favicon across a redeploy and resolve preload warnings.
  Precompression requires correct encoding negotiation and redeploy-safe caching.
- Verify `just hass::run` shuts down cleanly on SIGINT/SIGTERM without Just's interrupted error.

Ingress authenticates transport; the native adapter supplies actor context. Browser code has no storage/device access.
Persist no credentials. Close after the packaged add-on, hardware workflow, host snapshots, disconnects, and recovery
pass.
