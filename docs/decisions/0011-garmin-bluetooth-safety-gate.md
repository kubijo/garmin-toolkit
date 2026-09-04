# 0011: Garmin Bluetooth safety gate

## Decision

Direct Garmin Bluetooth is unsupported and non-gating. Passive discovery is allowed, but no connection, pairing,
registration, sync, acknowledgement, archive, upload, or erase may touch a data-bearing device.

Reconsider only on a disposable/reset device with complete before/after inventories. Prove GATT connection, OS bonding,
Garmin registration, reads, acknowledgements, writes, and destructive flags separately; each step needs approval.

## Why

The fēnix advertised no identifiable Garmin data while its phone was available and only a minimal manufacturer payload
when the phone was disabled. Garmin documents no safe multi-companion contract; adapter feasibility proves no protocol
safety.

## Consequences

If approved, ADR 0010 defines the split. Until disposable-device evidence exists, defer its crates, proxies, and macOS
work; use fakes and publishable fixtures.

See [Bluetooth adapter research](../research/bluetooth-adapters.md).
