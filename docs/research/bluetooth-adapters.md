# Bluetooth adapters

## Result

The adapter layer is feasible; Garmin Bluetooth remains barred by ADR 0011. If cleared, use
[`bluer` 0.17.4](https://docs.rs/bluer/0.17.4/bluer/) (BSD-2-Clause) on HASS/Linux and evaluate
[`btleplug` 0.12.0](https://docs.rs/btleplug/0.12.0/btleplug/) (MIT/Apache-2.0/BSD-3-Clause) on macOS.

## Contract constraints

- Identity is opaque and adapter-scoped; MAC addresses are optional.
- Discovery is owned and bounded. Filters are hints followed by post-filtering because BlueZ merges client filters.
- One backend owns a physical adapter in a process. Do not hand the same controller between `bluer` and `btleplug`.
- OS bonding and Garmin pairing are separate. `btleplug` lacks bonding controls exposed by `bluer`.
- Share adapter state, discovery, connection, GATT I/O, notifications, cancellation, and classified diagnostics; keep
  backend handles/errors private.

## Evidence

An ignored Rust 1.98.0 spike built both crates. Each scanned the same Linux controller without connection; consecutive
use in one process exposed a BlueZ discovery-stop race, while isolated runs passed.

Linux desktop and Raspberry Pi HASS expose powered central-role BlueZ controllers. HASS has `bluetoothctl`, not
`busctl`; no discovery identifiers were retained.

The watch advertised nothing identifiable while phone Bluetooth was on. With it off, one unnamed, unpaired Garmin
manufacturer advertisement appeared; no connection followed.

Further proof needs a disposable/reset device and complete inventories. macOS waits for Garmin protocol safety.

Historical source audit: AGPL-3.0 [Gadgetbridge 0.93.0][gb-audit] archives or acknowledges downloaded data in some sync
paths; that behavior is not inherited. AGPL-3.0-only
[garmin-bridge 0.1.0](https://github.com/wh1le/garmin-bridge/commit/5b849c7b81c005765560b7193d28c580ac80ca2c) uses BlueZ
but does not prove coexistence or history sync. Neither is a dependency or a substitute for the safety gate.

[gb-audit]: https://codeberg.org/Freeyourgadget/Gadgetbridge/commit/d5961f0e2bcde00aa201e34d11571ee005f58d86
