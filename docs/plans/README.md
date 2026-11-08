# Remaining work

This is the canonical work list. It contains unfinished outcomes, not completed acceptance or routine release steps.
[Architecture](../architecture/README.md) owns current contracts; [research](../research/README.md) owns evidence and
limits. Follow the [development workflow](../development.md) when taking an item. An unsupported case or measurement
limit is not automatically scheduled work.

## Local work

- Close the missing [FIT corpus cases](../research/fit-corpus.md#coverage-gaps): wellness/sleep/HRV, workouts, and
  developer fields. Test current acceptance, rejection, and original-byte preservation; broader normalization is a
  separate scope decision.
- Write the device-adapter extension guide around the [existing boundary](../architecture/connectivity.md), tests, and
  [support matrix](../research/device-capabilities.md#usb-operations).
- Assess battery, charging, and transport telemetry for portability, reliability, and sensitivity before selecting
  fields to expose. Hardware-dependent claims require observations from the target adapter/device.
- Implement the source-only Connect IQ companion and local SDK/key tooling described in
  [the research](../research/connect-iq-companion.md#tooling-to-implement). Compilation and device acceptance require
  owner-supplied inputs below; Garmin tools and outputs stay outside CI and releases.

## Hardware and owner access

- Complete Venu 3S explorer acceptance and establish Edge 850 support for manifest-declared operations. Record the
  device, firmware, host, and adapter; retain host/GVFS defect evidence.
- Qualify physical pairing-marker writes and reassignment.
- Verify live USB reconnect returns one usable attachment without duplicates.
- Qualify interrupted Course transfer and recovery, including firmware discovery afterward.
- Confirm the fēnix raw-MTP upload correction after a usable fresh session. The known `OpenSession` failure and
  successful GVFS map maintenance are separate evidence; see
  [USB limitations](../research/usb-sync.md#existing-adapters).
- Obtain firmware acceptance of workout delivery before claiming it is tested.
- Before advertising Windows browser support, verify popup retry/tab fallback, stale-popup rejection after parent
  reload/closure, and foreground focus with keyboard and touch. Activation must not recreate closed windows.
- Compile, sideload, and run the [Connect IQ probe](../research/connect-iq-companion.md#probe) once the local tooling,
  owner-supplied SDK/signing key, and HASS HTTPS endpoint exist. Record phone-present and phone-absent behavior.

The [device matrix](../research/device-capabilities.md) records established operations and their evidence.

## Upstream blockers

- Remove gallery-only RUSTSEC-2026-0370/0371/0372 exceptions when compatible published AccessKit/AT-SPI releases
  complete the [zbus 6 migration](https://github.com/z-galaxy/zbus/issues/2043). Preserve native accessibility. The
  [audit policy](../../infra/gallery/deny.toml) rejects obsolete exceptions and unrelated findings;
  [AccessKit's Rust support policy](https://github.com/AccessKit/accesskit/pull/834) explains the blocker.

## Deferred

- **Security hardening:** authentication, credential custody, database protection, and snapshot encryption are postponed
  in full, including feasibility experiments. [ADR 0023](../decisions/0023-deferred-security-hardening.md) defines
  current trusted/passwordless operation; [parked options](../research/security-hardening.md) are not decisions.
- **Garmin Connect and sharing:** resume after credential custody is established. Revalidate
  [ADR 0008](../decisions/0008-unofficial-garmin-connect-boundary.md), authentication, terms, and limits; then implement
  an opt-in adapter independent of USB. Preserve source provenance and application UUIDs. Choose sharing boundaries,
  authorization/revocation, conflict handling, and snapshot policy before implementation; test negative isolation.
- **Desktop hot reload:** revisit the [measured pauses](../research/build-performance.md#gallery-reload-measurement) on
  the selected loader version. Qualify retained UI state, callback/tracing lifetimes, compile-error recovery,
  first-visible-frame latency, and memory across repeated Linux/macOS reloads. Incompatible type edits require restart.
- **Wayland activation upstreaming:** retain the [winit patch](../../vendor/winit/PATCHES.md). Resume with real
  pointer/keyboard/touch, multi-seat and compositor focus evidence, an agreed initiating-window token API, and a
  compatible upstream branch. Synthetic input cannot supply a compositor serial.
- **In-app route planning:** routing-engine selection and geometry editing are outside the current GPX-to-Course
  workflow; see [ADR 0028](../decisions/0028-user-owned-route-plans.md).
- **Image baselines and capture coverage:** numeric coverage currently excludes UI views. Merging rendered capture
  coverage into LLVM profiles is not a current gate; see
  [coverage ownership](../architecture/build-system.md#checks-and-outputs).
- **Active Garmin Bluetooth:** requires separately approved disposable-device protocol safety evidence under
  [ADR 0011](../decisions/0011-garmin-bluetooth-safety-gate.md).

## Conditional maintenance

- Profile bindgen after representative WASM edits using the
  [build timing workflow](../architecture/build-system.md#development-build-timing).
- Before upgrading Rust, recheck the nightly `egui_adapter::Paint: Sync` recursion diagnostic against stable builds.
  Existing nightly measurements are
  [historical evidence](../research/build-performance.md#linker-and-compiler-evidence).

## Outside the current roadmap

A separate CLI distribution channel is not planned. Flatpak is an idea, not a release track. Mobile needs a separate
product decision. Devices without USB data access, including Index S2, are outside the current hardware scope.
