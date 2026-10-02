# Read-only device inspection

CLI, desktop, and HASS use `garmin-services::devices` for read-only inspection. Each volume retains separate namespace,
identity, and transaction results. Missing optional state is normal; malformed or unreadable sections retain errors.
Completed markers are reported without cleanup. Refresh keeps the previous snapshot, rejects stale attachment results,
and runs again after potentially mutating operations. The device page exposes summaries and expandable details.

Remaining hardware and deployment acceptance beyond [capacity](../research/device-capacity-and-recovery.md) and
[explorer](../architecture/device-explorer.md):

1. Verify adapter/device capacity matrix: identity, units, read-only media, disconnects, missing attributes, refresh.
   Keep volumes separate; do not crawl files for capacity.
2. Verify packaged HASS USB assignment, nested ingress, reconnect/disconnect, slow startup, and the actual DOM loader.
3. Compare CLI/desktop/HASS inspection and refresh on the same physical device.
4. Complete consented Venu 3S explorer acceptance. Simultaneous Edge/Venu attachment is not full proof; diagnose stale
   attachments at host/GVFS rather than hiding them. Follow
   [hardware safeguards](../development.md#execution-safeguards).

[Device expansion](device-expansion-and-publication.md) owns battery/transport-health research. Close when every claimed
client/adapter has refresh and hardware evidence.
