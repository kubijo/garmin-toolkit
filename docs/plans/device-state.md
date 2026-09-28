# Read-only device inspection

Remaining work beyond [capacity](../research/device-capacity-and-recovery.md) and
[explorer](../architecture/device-explorer.md):

1. Read exact `GARMIN-TOOLKIT` state paths independently, retaining section failures.
2. Define a canonical attachment/manifest/storage/identity/pairing/transaction snapshot; refresh on reconnect/mutation.
3. Verify adapter/device capacity matrix: identity, units, read-only media, disconnects, missing attributes, refresh.
   Keep volumes separate; do not crawl files for capacity.
4. Verify packaged HASS USB assignment, nested ingress, reconnect/disconnect, slow startup, and the actual DOM loader.
5. Prove CLI/desktop/HASS display and refresh the same snapshot.
6. Complete consented Venu 3S explorer acceptance. Simultaneous Edge/Venu attachment is not full proof; diagnose stale
   attachments at host/GVFS rather than hiding them. Follow
   [hardware safeguards](../development.md#execution-safeguards).

[Device expansion](device-expansion-and-publication.md) owns battery/transport-health research. Close when every claimed
client/adapter has refresh and hardware evidence.
