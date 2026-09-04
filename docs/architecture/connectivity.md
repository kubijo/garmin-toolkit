# Connectivity architecture

`garmin-device` owns consent-gated Garmin file access. Target adapters list attachments; a shared coordinator reconciles
initial state, arrivals, and departures because host events are not durable. Contents remain unread until consent.

`garmin-device` provides raw MTP through `mtp-rs` and Linux system mounts through GIO. Current CLI composition selects
either adapter; desktop and HASS attachment views use the system mount. A HASS add-on must compose raw MTP explicitly
because it cannot assume a desktop GIO session. Windows raw MTP uses the `mtp-rs` WPD backend. Adapter locations remain
private.

`DeviceLink` is the shared capacity and disposable-probe boundary for directories, desktop-mounted MTP, and raw MTP. Raw
benchmarks and raw transaction writes use one streaming primitive. Payload EOF completes `Upload`; waiting for the
device response is `DeviceFinalize`, so the displayed transfer rate cannot decay after all bytes were sent. The Garmin
split-transfer adapter holds back one byte for a final short USB transfer, guarding packet-aligned objects against the
missing streaming terminator in the pinned `mtp-rs` release.

Before a raw probe writes, it atomically records the device identity, storage, object name, size, and SHA-256 below the
configured cache. A final-response timeout is ambiguous rather than failed. The adapter keeps its exclusive session,
uses bounded recovery attempts to find and verify that exact object, and removes it. It reopens only after the transport
reports that the session ended. An unresolved object keeps its receipt and blocks the next probe for that device.

Raw link benchmarking owns one MTP session across manifest inspection, capacity validation, upload, verification, and
cleanup. It does not repeatedly open and close the responder between those phases. If a raw Garmin sends no response to
`OpenSession`, a whole-device USB reset may be attempted at most once before inspection is retried. Recovery reproduces
the transport's stable bus-and-port identity and checks it against the known Garmin VID/PID set, so it cannot reset an
arbitrary USB device. Identities for which retained hardware evidence disproved that recovery skip it and fail with a
physical-reconnect instruction. The rejected MTP class-reset request is not used, healthy sessions are not reset, and a
quiet interval separates any reset from the sole retry.

After consent, each storage is searched for canonical `GarminDevice.xml`. The shared parser exposes only allowlisted FIT
capabilities; transport handles and paths stay private. Unsafe hierarchies, incomplete listings, ambiguous manifests,
and changed candidates fail closed.

Files stream into caller-owned staging and must match the MTP size. Failures leave only a discardable unpublished prefix
and cancel the stream. Reads never delete device objects.

`garmin-map-service` owns Garmin's HTTP and JSON shapes. It accepts captured device XML and converts private wire DTOs
into `garmin-model::map`. `garmin-services::maps` owns download authorization policy and recovery orchestration;
`garmin-update` accepts only normalized map models, injected URL authorization, and `DeviceRead`/`DeviceWrite`. Neither
the wire schema nor `DeviceManifest` crosses into the update engine.

The device adapter normalizes installed map part numbers and versions from `GarminDevice.xml`. The map-service adapter
joins them to Garmin's catalog before clients receive installed-to-available comparisons.

An update approval retains selected catalog entries beside its executable plan. The confirmation shows aggregate file
and byte counts only after naming every component with its `Update` or `Install` action.

A selected activity directory is separate read-only ingress into the same artifact and parsing pipeline. Selection
grants access only to that directory; type, size, and parser limits still apply.

The physically proven fēnix VID/PID is a non-standard MTP quirk; standard Garmin MTP devices need no product table.
Tests cover manifest discovery, capability filtering, streaming, size checks, cancellation, hostile paths, and mounted
reads. Physical Linux evidence covers both GVFS and raw MTP reads.

Adapters provide snapshots and events; applications own scheduling. Consent UI, pairing, reconnect recovery, imports,
and writes sit above this boundary.

See [manifest-driven USB capabilities](../decisions/0012-manifest-driven-usb-capabilities.md) and
[USB synchronization evidence](../research/usb-sync.md).
