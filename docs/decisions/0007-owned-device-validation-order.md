# 0007: Owned-device validation order

## Decision

After the fēnix 8 Solar baseline, test the Venu 3S as a second watch, then the Edge 850 bike computer and Index S2
scale. Record each firmware/host/adapter/operation tuple.

## Why

All are available. Venu tests watch portability; Edge reuses USB/FIT while adding bike-computer semantics; Index uses
BLE setup and Wi-Fi sync, requiring separate provisioning, identity, cloud, and multi-user evidence.

## Consequences

No result is generalized from one watch or family. Scale work does not assume USB or ordinary Garmin file layout, and
later family order follows recorded hardware and capability evidence.

See [device capabilities](../research/device-capabilities.md).
