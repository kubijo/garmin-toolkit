# Home Assistant watch vertical slice

Finish GPX import and one confirmed FIT Course transfer on the packaged HASS host, then verify the device pairing flow
in the add-on and on physical media. [Shared workflows](shared-interface-workflows.md) owns desktop parity; physical
inspection evidence belongs to [device expansion](device-expansion-and-publication.md). Retain the native-host/WASM and
[typed service](../architecture/workspace.md) architecture; Bluetooth and HASS domain publication remain excluded.
Selected FIT import has [demo acceptance](../research/fit-implementation.md#browser-import-acceptance), including a
browser disconnect. Device reassociation has [demo acceptance](../research/usb-sync.md#demo-pairing-acceptance).

## Remaining work

1. Finish acceptance of the shared **Routes** workflow: GPX upload, exact candidate preview, explicit name/sport, saved
   route, versioned FIT Course generation, and original/Course downloads. The implementation uses the
   [route client contract](../architecture/storage.md#route-client-workflow); package and live acceptance remain open.
2. Transfer one selected Course artifact after preflight and confirmation; record readback and firmware acceptance.
   Cleanup requires separate consent. Verify disconnect/recovery without unintended mutation.
3. Integrate Home Assistant host backups with the managed deployment and verify recovery of its selected storage
   generation. Portable application snapshots have a separate
   [storage contract](../architecture/storage.md#portable-snapshots).
4. Build self-contained amd64/aarch64 add-ons with persistent `/data`; verify ingress-only mutation access, USB
   permissions/ownership, disconnects, and workflow on Raspberry Pi 5. Do not assume desktop GIO mounts exist. Verify
   [profile marker](../decisions/0018-on-device-profile-marker.md) initial pairing and reassociation in the add-on and
   on physical media; initial pairing has unit coverage but still needs live acceptance.

## GPX acceptance

The scope is import, review, and durable Course artifacts first; device transfer follows as a separately verified step.
The durable contracts live in [storage architecture](../architecture/storage.md#route-client-workflow),
[ADR 0028](../decisions/0028-user-owned-route-plans.md), and the
[parser containment notes](../research/gpx-route-plans.md#native-parser-containment).

Rebuild both host packages and confirm that the GPX helper is available beside the host executable. In desktop and HASS,
verify **Import GPX → select track/segment → preview and confirm name/sport → save route → generate Course → download**.
Include duplicate names, rejected siblings, unresolved controls, disconnected/reconnected clients, failed operations and
retries, profile switching, and restore with pending work. Confirm that downloaded original bytes are unchanged and
downloaded Course bytes match the selected persisted version. Check narrow layouts and pointer/keyboard affordances.
Import and download must work without an attached device and never mutate one.

Fresh demo data must show the recorded walking and hiking activities from the
[development corpus](../research/fit-corpus.md#development-corpus), with correct sport labels, filters, maps, and
charts. Alex must also start with the saved cycling, walking, and hiking routes exported from that corpus. Repeated
seeding must remain idempotent and production initialization must remain empty or preserve existing data.

### Device transfer and acceptance

Only after import, generation, and download are accepted, add transfer of a selected persisted Course artifact. Reuse
device inspection, capability evidence, write exclusion, cancellation, and recovery from shared device workflows.
Preflight binds the profile, artifact digest/version, device identity, target storage, and create-only filename.
Confirmation must be invalidated by a changed target or artifact. Do not expose a generic write-path RPC.

Persist transfer intent before mutation and reconcile interrupted writes using the exact recorded destination and
digest. Readback records transfer verification; firmware rediscovery or explicit user confirmation records acceptance
separately. Cleanup remains a separately approved operation restricted to this transfer's partial upload. Test with the
demo device first; an owned-device trial needs explicit authorization and capability evidence.

### Validation before closing each step

- GPX 1.0/1.1, multiple tracks/segments, rejected siblings, empty/malformed/oversized input, optional elevation,
  unsupported extensions retained in original bytes, and unresolved controls.
- Walking, hiking, running, and cycling remain distinct through import, persistence, FIT encode/decode, download, and
  snapshot restore; migration preserves existing running/cycling routes.
- Fresh desktop/HASS demo data contains recorded walking and hiking activities with the correct sport, route, and
  metrics. Repeated seeding adds no duplicates, existing cases remain present, and production is never demo-seeded.
- Ownership isolation, stale/foreign preview IDs, changed selections, concurrent confirmation, response loss,
  cancellation, expiry, and no mutation before confirmation.
- Confirmation cannot bypass parser containment. Retry a committed import after preview expiry, host restart, and a
  later route revision; return the original receipt without moving the head. Changed arguments conflict. A failed import
  leaves no partial source and can be retried while its preview is live.
- Restore during preview, confirmation, generation, and download; old jobs cannot mutate the replacement storage and
  foreign-profile artifact IDs cannot obtain download tickets.
- HASS native/WASM checks and demo browser acceptance for import, review, generation, download, and reconnect. Then add
  transfer failure/recovery tests and separately record hardware acceptance.

Use the shared UI for desktop parity; the browser never acquires filesystem or device access. Keep the implementation
source-neutral and add no Mapy.com account/API dependency. Move completed contracts to architecture/ADR owners and
remove the corresponding plan sections rather than preserving a completed task log.

Complete focused native/WASM checks, adversarial review, and packaged demo acceptance before committing the vertical
slice. Heavy builds and host launches need explicit approval; give the exact command when a rebuilt host is required.

## Browser/process acceptance

- Inspect the actual DOM loader under slow/cached/unknown-size/failed/stale-bundle responses and nested ingress.
- Verify normal reload receives current entrypoint/favicon across a redeploy and resolve preload warnings.
  Precompression requires correct encoding negotiation and redeploy-safe caching.
- Verify `just hass::run` shuts down cleanly on SIGINT/SIGTERM without Just's interrupted error.

Ingress authenticates transport; the native adapter supplies actor context. Browser code has no storage/device access.
Persist no credentials. Close after both packages build, route revisions reproduce previews/FIT, and the hardware
workflow, snapshots, disconnects, and recovery pass.
