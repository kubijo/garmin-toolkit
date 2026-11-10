# Local Garmin activity downloads

## Outcome and boundary

Retrieve complete saved activity FIT files over Garmin's native Bluetooth protocol, retain their original bytes, and
feed the existing local FIT import path. Start with Linux and the Edge 1050; qualify other device/firmware tuples
separately. Use Protobuf FileAccess and MultiLink transfer pipes, without a legacy file-index fallback. No Connect IQ
app or Garmin Connect account is required.

The [download command](../research/bluetooth-adapters.md#fileaccess-download-command) is implemented and tested against
synthetic peers. Physical use remains subject to [ADR 0011](../decisions/0011-garmin-bluetooth-safety-gate.md); this
plan does not authorize a data-bearing device connection. Phone coexistence remains unqualified.

## Ownership and source

[garmin-gfdi](../../crates/garmin-gfdi/README.md) owns wire types and offline state machines, with
[pinned FileAccess schemas](../../crates/garmin-gfdi/proto/README.md). Its source record and the
[adapter audit](../research/bluetooth-adapters.md#gadgetbridge-source-basis) identify adapted Gadgetbridge behavior,
provenance, and excluded device-state changes.

`garmin-device` owns the optional Linux transport and private smoke artifacts. Application retention and import belong
in `garmin-services`. Ordinary discovery must not start native Bluetooth sessions.

## Remaining

1. **Physical qualification.** Qualify the
   [first-pairing command](../research/bluetooth-adapters.md#first-pairing-command) and Garmin setup acknowledgements.
   After the separately approved gate, compare a pulled activity byte-for-byte with its USB/MTP copy. Verify full
   unfiltered pagination, repeated reads, interruption/restart, changing-file behavior, unchanged
   inventories/flags/bonds, and Garmin Connect's subsequent ability to sync. Retain exact firmware, route, and host
   versions. Reject a route whose effects remain unexplained.
2. **Import handoff.** Feed the validated original FIT artifact to the existing import service, retaining device/item
   provenance and deduplicating through existing behavior. Protocol/cleanup/FIT failures must not enter application
   storage. Restart interrupted smoke runs from zero with fresh captures; resumable transfers, background syncing, HASS
   publication and upload are outside this slice.
