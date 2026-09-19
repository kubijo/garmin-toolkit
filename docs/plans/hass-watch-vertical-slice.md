# Home Assistant watch vertical slice

Import preserved FIT from the recorded watch, render it in egui/WASM, create route plans, and deploy one FIT Course.
Attaching or mounting a recognizable Garmin authorizes bounded local inspection. File transfer and writing require
separate confirmation. Bluetooth is excluded. The native HASS process owns storage, devices, sync, API, and assets;
ingress serves WASM without credentials or host-device access.

[Shared interface workflows](shared-interface-workflows.md) owns user-visible parity with desktop. This plan owns the
HASS host, typed browser boundary, packaging, deployment, and hardware proof.

## Shared host foundation

Keep the native-server/WASM deployment and [typed service boundary](../decisions/0024-remoc-service-boundary.md); JSON
polling is not a replacement decision.

Continue from the [validated device-capacity foundation](../research/device-capacity-and-recovery.md):

1. Extend `garmin-service-api` with opaque plan and job IDs. Local and remote clients must call `garmin-services`; never
   accept caller-selected host paths or adapters.
2. Wire catalog loading, selection, exact-plan approval, progress, cancellation, and recovery through the existing map
   service. Retain completed history across browser reconnects. Neither reconnect nor stale approval may replay a write.
   Use the same flow with physical or virtual device adapters.
3. Test the loader under slow, cached, unknown-size, failed, and stale-bundle responses. Capture the actual DOM loader;
   shared GUI gallery scenes do not prove browser or ingress behavior. Add precompressed assets only after the host
   selects encodings correctly and retains redeploy-safe cache handling.
4. Build self-contained amd64 and aarch64 add-on bundles with persistent `/data`. Keep the API ingress-only and verify
   its trust boundary before enabling mutations. Compose host device access through existing traits; do not assume
   desktop GIO mounts exist inside the add-on. Prove permissions, ownership, and disconnects on the Raspberry Pi 5.

The watch workflow below builds on the existing device capacity and map-host boundary.

## Browser and process acceptance

- Verify a normal reload receives the favicon and current entrypoint without DevTools cache bypass. Keep HTML and the
  initializer `no-store` and content-hashed static assets immutable.
- Remove or justify duplicate/unused WASM preload warnings. Loader prose must describe actual download/startup phases.
- Verify `Ctrl+C` through `just hass::run` exits cleanly after application-owned SIGINT/SIGTERM shutdown, without Just
  reporting `error: interrupted by SIGINT`.

## Ordered proof

01. Detect attachments and automatically inspect manifest and storage metadata.
02. Pair a profile through the on-device marker; prove verified creation, updates, reconnect, and reassociation.
03. Import selected FIT read-only with interruption recovery and idempotent retries.
04. Extend the shared service boundary to watch import and route operations.
05. Extend the existing [proxied vector map](../architecture/activity-map.md) to route-plan editing and overlays.
06. Complete [route planning](../decisions/0028-user-owned-route-plans.md): GPX selection, freehand geometry, revisions,
    reverse, trim, split, and geographic simplification.
07. Prove preflight and recovery, then transfer one FIT Course with confirmation, readback, acceptance status, and
    separately consented cleanup.
08. Resolve OQ-022 behind the route interface.
09. Integrate host backup and [portable snapshots](data-foundation.md).
10. Validate the watch workflow on the [HASS targets](../decisions/0026-hass-architecture-and-bluetooth-scope.md).

Ingress authenticates transport, not application users. The native adapter supplies actor context; browser code cannot
reach storage or devices. Persist no credentials and publish no domain data to HASS under ADR 0027.

Delete this plan after the watch workflow survives disconnect and resume without unintended mutation; route revisions
reproduce previews and FIT output; one transfer has byte and device-acceptance evidence; snapshots and failure paths are
tested; the Raspberry Pi 5 run passes; and both claimed artifacts build.
