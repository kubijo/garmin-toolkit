# Home Assistant watch vertical slice

Finish GPX import and one confirmed FIT Course transfer on the packaged HASS host, then verify the device pairing flow
in the add-on and on physical media. [Shared workflows](shared-interface-workflows.md) owns desktop parity; physical
inspection evidence belongs to [device expansion](device-expansion-and-publication.md). Retain the native-host/WASM and
[typed service](../architecture/workspace.md) architecture; Bluetooth and HASS domain publication remain excluded.
Selected FIT import has [demo acceptance](../research/fit-implementation.md#browser-import-acceptance), including a
browser disconnect. Device reassociation has [demo acceptance](../research/usb-sync.md#demo-pairing-acceptance).

## Remaining work

1. Expose selected GPX import through `garmin-service-api` and `garmin-services`: bounded file ingress, candidate
   preview, explicit track/segment selection, name and sport, and preservation of the original. Use host-issued IDs;
   reject caller-selected host paths and adapters. Show the exact track geometry before generating a FIT Course. GPX
   routes containing only control points need routing or an explicit straight-line decision and cannot be silently
   converted into a Course. Persist each generated FIT Course as a separate versioned artifact linked to the selected
   route revision, so it can be downloaded or sent to a device later. Do not build an in-app route planner for this
   slice.
2. Transfer one selected Course artifact after preflight and confirmation; record readback and firmware acceptance.
   Cleanup requires separate consent. Verify disconnect/recovery without unintended mutation.
3. Integrate Home Assistant host backups with the managed deployment and verify recovery of its selected storage
   generation. Portable application snapshots have a separate
   [storage contract](../architecture/storage.md#portable-snapshots).
4. Build self-contained amd64/aarch64 add-ons with persistent `/data`; verify ingress-only mutation access, USB
   permissions/ownership, disconnects, and workflow on Raspberry Pi 5. Do not assume desktop GIO mounts exist. Verify
   [profile marker](../decisions/0018-on-device-profile-marker.md) initial pairing and reassociation in the add-on and
   on physical media; initial pairing has unit coverage but still needs live acceptance.

## Browser/process acceptance

- Inspect the actual DOM loader under slow/cached/unknown-size/failed/stale-bundle responses and nested ingress.
- Verify normal reload receives current entrypoint/favicon across a redeploy and resolve preload warnings.
  Precompression requires correct encoding negotiation and redeploy-safe caching.
- Verify `just hass::run` shuts down cleanly on SIGINT/SIGTERM without Just's interrupted error.

Ingress authenticates transport; the native adapter supplies actor context. Browser code has no storage/device access.
Persist no credentials. Close after both packages build, route revisions reproduce previews/FIT, and the hardware
workflow, snapshots, disconnects, and recovery pass.
