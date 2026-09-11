# Read-only device inspection

[Capacity behavior and validation](../research/device-capacity-and-recovery.md) are complete. This plan owns the
remaining device-state work.

## Implemented

- Mounted devices automatically expose manifest and storage state in desktop and HASS. Inspection does not run the file
  catalog. Transfer, networking, and mutation remain explicit.

## Remaining work

1. Read the exact `GARMIN-TOOLKIT` state paths independently and preserve section-specific failures.
2. Define one canonical snapshot containing attachment, manifest, storage, application identity, pairing, and pending
   transaction state. Refresh it after attachment, reconnect, and mutation; preserve section-specific failures.
3. Record an adapter/device applicability matrix. Verify capacity on every supported combination and mark unavailable
   combinations explicitly. Check storage identity, units, read-only media, disconnects, missing attributes, and
   refresh. Capacity queries must not crawl device files.
4. Validate the packaged HASS host and browser under USB assignment, nested ingress, reconnect, disconnect, and slow
   startup. Capture the real DOM loader separately from the shared `garmin-ui` gallery evidence.
5. Extend the HASS browser from its device-state page into the full shared application UI, then prove that CLI, desktop,
   and HASS render the same canonical snapshot and refresh it after reconnect or mutation. A planned-space segment is
   optional; separate volumes remain mandatory.

[USB synchronization](../research/usb-sync.md#map-maintenance-evidence) records completed transaction checks and
hardware write evidence. [Device expansion](device-expansion-and-publication.md) owns optional battery and
transport-health research. Delete this plan once every claimed client displays refreshed state and hardware results
establish each supported adapter.
