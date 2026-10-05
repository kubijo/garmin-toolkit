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

## Proposed GPX implementation sequence

Review this sequence before implementation. The scope is import, review, and durable Course artifacts first; device
transfer follows as a separately verified step.

### Import and preview

Reuse `garmin-gpx` candidate parsing, `garmin-model::route`, and `Storage::save_route_import`; do not create a parallel
route store. Add portable contracts in `garmin-service-api::routes` and orchestration in `garmin-services::routes`. HASS
supplies the authenticated actor and selected profile; services enforce ownership on every operation.

- Accept bounded GPX bytes and a display filename. Enforce the existing 16 MiB byte and one-million-point limits, plus
  bounded pending uploads, candidate counts, preview payloads, and expiry. Review XML expansion/allocation before
  parsing; checking a point count after parsing alone is not a complete memory bound.
- Return an opaque, host-issued preview ID bound to the owner and exact input digest. Candidate identities select one
  original track/segment or unresolved route; never accept client-supplied geometry, host paths, or parser selection.
- Present valid and rejected candidates separately. Require a name and sport, even when metadata provides suggestions.
  Preview the selected exact geometry. Control-only routes explain why Course generation is unavailable in this slice.
- Confirmation consumes the preview and atomically persists original bytes, acquisition provenance, route plan, and
  initial revision. Bind retries to a host-issued operation ID so a lost response cannot create duplicate plans.
  Cancel/expiry releases pending bytes. A disconnected browser must not retarget work to another profile.

### Immutable Course artifacts

The existing FIT encoder accepts a route revision and assigned serial number. Add a generated-Course record linking an
immutable artifact to its route revision, encoder identity/version, generation time, and serial. Give each generation
its own identity and an ordered version within that revision; identical bytes may share a blob but not erase generation
history. Use a forward migration with ownership/reference constraints and atomic artifact/record writes.

Generate only from persisted exact geometry after validation. Keep FIT coordinate/elevation quantization explicit in
verification; preserve the original route coordinates and reject values the encoder cannot represent. Do not invent turn
cues. Allocation of a fresh serial and generation version must be safe under concurrent requests and retries.

List saved routes and their generated versions. A download selects a specific artifact ID through the existing
single-use download-ticket mechanism; it must return persisted bytes rather than silently regenerate them. A failed
generation leaves the saved route available for review/retry. Regeneration creates a new artifact; old downloads and
future transfer records remain bound to the original version. Portable backup/restore must include these records.

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
- Ownership isolation, stale/foreign preview IDs, changed selections, concurrent confirmation, response loss,
  cancellation, expiry, and no mutation before confirmation.
- Immutable generation history, concurrent serial/version allocation, exact-byte download after restart, encoder
  failure, transaction rollback, FIT decode comparison within its numeric precision, and snapshot round trips.
- HASS native/WASM checks and demo browser acceptance for import, review, generation, download, and reconnect. Then add
  transfer failure/recovery tests and separately record hardware acceptance.

Use the shared UI for desktop parity; the browser never acquires filesystem or device access. Keep the implementation
source-neutral and add no Mapy.com account/API dependency. Move completed contracts to architecture/ADR owners and
remove the corresponding plan sections rather than preserving a completed task log.

## Browser/process acceptance

- Inspect the actual DOM loader under slow/cached/unknown-size/failed/stale-bundle responses and nested ingress.
- Verify normal reload receives current entrypoint/favicon across a redeploy and resolve preload warnings.
  Precompression requires correct encoding negotiation and redeploy-safe caching.
- Verify `just hass::run` shuts down cleanly on SIGINT/SIGTERM without Just's interrupted error.

Ingress authenticates transport; the native adapter supplies actor context. Browser code has no storage/device access.
Persist no credentials. Close after both packages build, route revisions reproduce previews/FIT, and the hardware
workflow, snapshots, disconnects, and recovery pass.
