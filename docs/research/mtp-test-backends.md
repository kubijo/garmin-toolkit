# MTP test backends

Investigated 2026-09-05. Directory-backed simulation is acceptable; no device or mount is needed for protocol tests.

## Selected backend

Reuse the pinned `mtp-rs` 0.32.0 `virtual-device` feature beneath the normal MTP/PTP client. Objects live in temporary
directories. It supports multiple volumes, read-only storage, capacity, optional filesystem events, and reopening
registered locations. See [upstream usage](https://docs.rs/crate/mtp-rs/0.32.0). Storage details:
[configuration](https://docs.rs/crate/mtp-rs/0.32.0/source/src/transport/virtual_device/config.rs).

The existing test dependency already enables this feature. Tests in `garmin-device/tests/virtual_mtp.rs` exercise
manifest parsing, upload/readback/reopen/deletion, production inventory, and read-only refusal. These are protocol
integration tests, not complete CLI or GIO/GVfs tests.

Upstream [fault hooks](https://docs.rs/crate/mtp-rs/0.32.0/source/src/transport/virtual_device/registry.rs) cover
partial reads, reset/cancel failures, and undescribable objects. Inject remaining faults through deterministic trait
decorators; reuse the transaction engine and MTP implementation.

## Runtime adoption

[ADR 0039](../decisions/0039-virtual-device-runtime.md) selects this backend for demo and dry-run, not tests alone. Demo
starts from synthetic state; dry-run shadows affected state from the selected device and retains real service calls.
Both execute the shared transaction engine through `garmin-services::UpdateTarget`. Demo and dry-run pass read-only
source capabilities to `SimulatedTarget`; it snapshots affected state into a retained virtual MTP device, then gives
that device to the normal transaction engine. `PhysicalTarget` supplies the same boundary for real commits.

Directory backing also supplies the retained inspection artifact: preserve the actual shadow tree, affected originals,
and observed changes under the capture. Disposable test directories must not dictate runtime artifact lifetime.

## Alternatives assessed

- [SwiftMTP's virtual device](https://github.com/EffortlessMetrics/SwiftMTP-dev/blob/main/SwiftMTPKit/Sources/SwiftMTPTestKit/VirtualMTPDevice.swift)
  holds objects in memory but implements Swift's application protocol. It is not a responder for the Rust MTP client.
- [uMTP-Responder](https://github.com/viveris/uMTP-Responder) uses Linux USB gadget interfaces and filesystem storage.
  It is not an in-process, cross-platform test dependency.
- [cmtp-responder's virtual setup](https://www.collabora.com/news-and-blog/blog/2024/06/12/cmtp-responder-news/) uses
  `dummy_hcd` and optionally QEMU. Consider it for isolated Linux/GVfs conformance, not the fast test tier.

No drop-in, fully memory-backed Rust responder was found. Upstream's scripted `MockTransport` is test-private and is not
a stateful device. Temporary-directory backing preserves the existing protocol implementation.

## Test boundaries

Application E2E must drive the CLI from selection to final output, including service parsing, planning, downloads,
authorization, and commit/recovery. Inject synthetic endpoints and device discovery. Virtual registrations use synthetic
VID/PID values; production Garmin filters must remain intact.

An in-process virtual registry does not automatically reach a child CLI process. Supply test composition explicitly,
without introducing test workflow branches. Keep durable capture/storage in disposable directories for restart tests.

Native adapter conformance remains separate: a virtual raw-MTP responder does not exercise Linux GIO/GVfs, macOS
ownership, Windows WPD, USB timing, or Garmin firmware. A tmpfs-backed directory does not change that boundary. Current
physical upgrade, restart, and process-interruption evidence is recorded in
[USB synchronization](usb-sync.md#map-maintenance-evidence).
