# 0010: Bluetooth adapter boundaries

## Decision

When needed, create `bluetooth` for the central-GATT contract, `bluetooth-bluez` for HASS/Linux, and
`bluetooth-btleplug` as an unproven macOS candidate. `garmin-ble` depends only on the contract. Each app selects one
backend per adapter; backends are separate crates, not features.

The contract uses opaque adapter-scoped identities, bounded owned discovery, post-filtering, and explicit optional OS
bonding. Garmin pairing stays in `garmin-ble`.

## Why

BlueZ exposes pairing and diagnostics; `btleplug` offers portable GATT but lacks pairing, macOS MAC addresses, and an
object-safe boundary. A Linux probe ran both and found a discovery race when they shared one controller.

## Consequences

Linux retains BlueZ controls; macOS identity semantics stay in its backend. Shared tests use a fake; conformance remains
target-specific.

See [Bluetooth adapter research](../research/bluetooth-adapters.md).
