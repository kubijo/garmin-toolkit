# Extending device adapters

Start with the [connectivity contract](architecture/connectivity.md). A new device model using an existing transport
usually needs synthetic manifest coverage and physical acceptance, not a new adapter or a product-ID entry. Add a quirk
only for a demonstrated transport difference; manifest capabilities are not proof that firmware accepts an operation.

## Discovery and inspection

Implement [`attachments::Backend` and `Candidate`](../crates/garmin-device/src/attachments.rs) inside `garmin-device`.
`candidates()` returns the current recognizable attachments without reading contents; `poll_changed()` reports stale
discovery state. Keep transport handles private and candidate keys stable for the current attachment. Return bounded
metadata from `inspect()`; implement browsing only for explicit explorer requests.

Use `attachments::Manager` for reconciliation, inspection generations, refresh, and detach handling. Do not create a
second lifecycle coordinator. The [mounted](../crates/garmin-device/src/mounted_catalog.rs) and
[raw](../crates/garmin-device/src/raw_catalog.rs) implementations show the concrete adapter shape.

Reuse [`parse_manifest`](../crates/garmin-device/src/manifest/mod.rs). A new metadata format needs detection by root and
namespace, a separate wire parser, and normalization into the existing model. Keep format-specific fields and transport
paths out of application contracts; adding a parser alone does not add another manufacturer's device operations.

## File access and composition

Implement only the required [storage boundaries](../crates/garmin-device/src/storage.rs): `DeviceRead` for inspection,
inventory, bounded reads, backup, and verification; `DeviceWrite` for authorized mutations; `DeviceLink` for capacity
and disposable probes. The [directory](../crates/garmin-device/src/storage/directory.rs) and
[raw-MTP](../crates/garmin-device/src/storage/raw_mtp.rs) implementations are examples.

Validate `SafeRelativePath` before accessing transport objects. Reject ambiguous manifests and changed candidates; check
read sizes, honor cancellation, and publish staged files only after verification. Writes need explicit authority and
recovery for ambiguous completion; reuse the transaction workflow rather than treating a timeout as a safe retry.

Select the adapter at the application boundary. Desktop uses
[`device_backend::production`](../apps/garmin-desktop/src/device_backend/production.rs); HASS uses its
[mounted](../apps/garmin-hass/src/devices/mounted.rs) and [raw](../apps/garmin-hass/src/devices/raw.rs) compositions.
Applications retain scheduling, consent, pairing, imports, and update policy.

## Verification and support claims

- Extend [manifest tests](../crates/garmin-device/tests/capabilities.rs) with synthetic XML for accepted capabilities,
  unsupported formats, unsafe paths, and ambiguous or changed manifests.
- Follow the lifecycle tests in `attachments.rs`: initial inventory, arrival, detach/reconnect, coalesced refresh, and
  rejection of stale inspection results.
- Exercise streaming, cancellation, size mismatch, and source changes using
  [mounted reads](../crates/garmin-device/src/read_mounted.rs), [MTP reads](../crates/garmin-device/src/read_mtp.rs),
  and the [virtual MTP responder](../crates/garmin-device/tests/virtual_mtp.rs). For writes, cover verification,
  interrupted completion, recovery, and cleanup without touching physical devices in automated tests.

Run `just in-shell cargo test --locked --offline -p garmin-device` and the applicable repository QA checks under the
[development safeguards](development.md#execution-safeguards). Physical tests require explicit authorization. Record
device, firmware, host, adapter, and individual proven operations in the
[support matrix](research/device-capabilities.md#usb-operations), with detailed evidence in the relevant research page.
Keep synthetic test success separate from physical acceptance; use an approved anonymization procedure for real
captures.
