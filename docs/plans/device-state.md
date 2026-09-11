# Read-only device inspection

[Implemented capacity behavior and validation](../research/device-capacity-and-recovery.md) are durable. This plan
tracks only unsupported metadata and runtime proof.

## Remaining work

1. Record an adapter/device applicability matrix. Verify capacity on every supported combination and mark unavailable
   combinations explicitly. Check storage identity, units, read-only media, disconnects, missing attributes, and
   refresh. Capacity queries must not crawl device files.
2. Validate the packaged HASS host/browser connection under nested ingress, reconnect, disconnect, and slow startup.
   Capture the real DOM loader separately from the shared `garmin-ui` gallery evidence.
3. Prove that CLI, desktop, and HASS render the same canonical snapshot after consent and refresh it after reconnect or
   mutation. A planned-space segment is optional; separate volumes remain mandatory.

[USB synchronization](../research/usb-sync.md#map-maintenance-evidence) records completed transaction checks and
hardware write evidence. [Device expansion](device-expansion-and-publication.md) owns optional battery and
transport-health research. Delete this plan once every claimed client displays refreshed state and hardware results
establish each supported adapter.
