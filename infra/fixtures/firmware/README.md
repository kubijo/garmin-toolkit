# Garmin firmware lookup fixtures

Response bodies from Garmin software update lookups. The captured requests returned HTTP 200.

Each lookup JSON has a matching synthetic XML inventory with the same filename stem. The two additional XML fixtures
exercise storage routing for a repeated part number and ambiguity when both entries name the same destination.

## Edge 1050

- `edge-1050-32.20.json` uses the installed software version.
- `edge-1050-31.29.json` uses a simulated older version.

Both responses offer the 33.22 software bundle and three other updates. The bundle has `isPrimaryFirmware: false`, so
use this case when testing how the UI identifies the device update.

## fēnix 8 Solar, 47 mm

- `fenix-8-solar-47mm-24.20.json` uses the installed software version.
- `fenix-8-solar-47mm-22.44.json` uses a simulated older version.

Both responses offer mobility and move alert videos, a Noto Sans Math font, and a separate `UpdateMetadata` entry for
each. Neither response includes a system software offer. Use these cases to test a catalog with component updates and
metadata entries.

`fenix-8-solar-47mm-gsp-22.44.json` changes only the installed version of bundle part `006-V4532-00` in the manifest's
`UpdateFile` inventory. Garmin adds the 24.20 system bundle and its metadata offer. Changing only the `GUPDATE.GCD`
entry did not produce a system offer.

`fenix-8-solar-47mm-gsp-metadata.json` is the matching metadata payload. Its size and MD5 matched the offer. The part
number, source version, and destination version describe the bundle transition. The video and font metadata fixtures use
the same fields. Compact JSON in the supplied field order reproduces the advertised size and MD5 of each of the three
payloads.

The loopback simulator serves small inert binaries and the compact metadata records. Its responses advertise the local
URLs, sizes and MD5 checksums for those bytes. The stored catalogs retain the captured sizes for display tests.

## Redaction

Response URLs use the reserved `example.invalid` domain. The XML inventories use fictional unit IDs and omit unlock
codes. Only the fields needed for the catalog tests appear in them.
