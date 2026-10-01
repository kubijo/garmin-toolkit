# Home Assistant watch vertical slice

Deliver watch FIT import, route editing, and one confirmed FIT Course transfer on the packaged HASS host.
[Shared workflows](shared-interface-workflows.md) owns desktop parity; [device inspection](device-state.md) owns
canonical attachment/capacity state. Retain the native-host/WASM and
[typed service](../decisions/0024-remoc-service-boundary.md) architecture; Bluetooth and HASS domain publication remain
excluded.

## Remaining work

1. Add opaque plan/job IDs and watch import/route operations to `garmin-service-api` through `garmin-services`. Reject
   caller-selected host paths/adapters.
2. Complete [map workflow runtime acceptance](shared-interface-workflows.md#map-workflow-acceptance) in the packaged
   host, including disconnects, retained outcomes, device exclusion, and recovery after restart.
3. Verify marker pairing/reassociation and selected FIT import with interruption recovery and idempotent retries.
4. Extend the map to [route-plan editing](../decisions/0028-user-owned-route-plans.md): GPX selection, freehand
   geometry, revisions, reverse/trim/split/simplification. Resolve
   [routing](open-questions.md#oq-022-route-plan-routing).
5. Transfer one FIT Course after preflight and confirmation; record readback and firmware acceptance. Cleanup requires
   separate consent. Verify disconnect/recovery without unintended mutation.
6. Integrate Home Assistant host backups with the managed deployment and verify recovery of its selected storage
   generation. Portable application snapshots have a separate
   [storage contract](../architecture/storage.md#portable-snapshots).
7. Build self-contained amd64/aarch64 add-ons with persistent `/data`; verify ingress-only mutation access, USB
   permissions/ownership, disconnects, and workflow on Raspberry Pi 5. Do not assume desktop GIO mounts exist.

## Browser/process acceptance

- Inspect the actual DOM loader under slow/cached/unknown-size/failed/stale-bundle responses and nested ingress.
- Verify normal reload receives current entrypoint/favicon, without DevTools cache bypass; resolve preload warnings.
  Precompression requires correct encoding negotiation and redeploy-safe caching.
- Verify `just hass::run` shuts down cleanly on SIGINT/SIGTERM without Just's interrupted error.

Ingress authenticates transport; the native adapter supplies actor context. Browser code has no storage/device access.
Persist no credentials. Close after both packages build, route revisions reproduce previews/FIT, and the hardware
workflow, snapshots, disconnects, and recovery pass.
