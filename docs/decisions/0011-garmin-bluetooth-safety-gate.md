# 0011: Garmin Bluetooth safety gate

## Decision

Garmin's private Bluetooth sync protocol is unsupported and non-gating. Passive discovery is allowed, but no connection,
pairing, registration, sync, acknowledgement, archive, upload, or erase through that protocol may touch a data-bearing
device.

Reconsider only on a disposable/reset device with complete before/after inventories. Prove GATT connection, OS bonding,
Garmin registration, reads, acknowledgements, writes, and destructive flags separately; each step needs approval.

A project-owned Connect IQ app connecting to our custom host GATT service is a separate experiment. It may be tested on
an owned data-bearing watch with explicit approval for sideloading and a bounded telemetry run; a reset is not required.
The run must not invoke Garmin's private sync protocol, alter OS bonds or Garmin Connect registration, or write
activity, course or workout data. Record watch/host versions and phone pairing/sync behavior before and after. Stop on
unexpected pairing prompts or phone connection disruption. Phone coexistence remains unqualified until tested.

## Why

The fēnix advertised no identifiable Garmin data while its phone was available and only a minimal manufacturer payload
when the phone was disabled. Garmin documents no safe multi-companion contract; adapter feasibility proves no protocol
safety.

## Consequences

Until disposable-device evidence exists, defer native Garmin-protocol adapters, proxies, and macOS Bluetooth work.
Project-owned Connect IQ code may be implemented and tested against fakes and private D-Bus daemons without physical
approval. Physical runs follow the separate Connect IQ scope above; fake tests do not establish phone coexistence.
[Adapter research](../research/bluetooth-adapters.md) informs a future design but grants no permission to connect or
write.
