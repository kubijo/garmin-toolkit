# Device capabilities

Support is keyed by device/firmware/host/adapter/operation; untested cells are unknown.

## Validation baseline

| Field            | Value                                                   |
| ---------------- | ------------------------------------------------------- |
| Device           | Garmin fēnix 8 Solar, 47 mm                             |
| Firmware         | Manifest software version `2244`; verify UI rendering   |
| Existing pairing | Paired with the owner's phone                           |
| Host             | Raspberry Pi 5, `aarch64`, Home Assistant OS 18.2       |
| Bluetooth        | Onboard Broadcom BCM43438 over UART through local BlueZ |
| Proxy            | None                                                    |
| USB expectation  | MTP over a direct Raspberry Pi connection               |

USB validation preserves phone pairing; ADR 0011 prohibits active Bluetooth. Size identifies the test row, not a
protocol split.

## Validation hardware

| Role              | Exact device         | Family        | Availability | Documented local paths                          |
| ----------------- | -------------------- | ------------- | ------------ | ----------------------------------------------- |
| Baseline          | fēnix 8 Solar, 47 mm | Watch         | Available    | Bluetooth, Wi-Fi, MTP/Garmin USB mode           |
| Watch conformance | Venu 3S              | Watch         | Available    | Bluetooth and Wi-Fi; verify USB mode physically |
| Second family     | Edge 850             | Bike computer | Available    | Bluetooth, Wi-Fi, MTP file transfer             |
| Bike conformance  | Edge 1050            | Bike computer | Available    | Bluetooth, Wi-Fi, MTP file transfer             |

Venu tests watch portability; Edge 850 establishes bike support and Edge 1050 tests conformance. Record firmware and
host for every run.

Index S2 is available but deferred with other devices lacking USB data access under ADR 0016.

Sources:
[Edge 850 computer connection](https://www8.garmin.com/manuals/webhelp/GUID-5BA20A50-BFFF-4418-AE4E-CA719C39EB05/EN-US/GUID-B4663AC8-EEF9-4684-883E-3CA79B0351EB.html),
[Edge 1050 file transfer](https://www8.garmin.com/manuals/webhelp/GUID-08ACA9FC-DEE6-4C8D-8A95-F62181C512E9/EN-US/GUID-50017E96-C40C-471D-BE61-3A0C09E93916.html),
[Index S2 setup](https://www8.garmin.com/manuals/webhelp/GUID-0BADEBCF-960D-4961-856C-86B9204BC169/EN-US/GUID-4477BB33-A532-4DE8-B8EC-DC28F6349AB4.html),
and [Index S2 specifications](https://www.garmin.com/en-GB/p/679362/pn/010-02294-70/).

## Operations

| Passive discovery                      | Pairing/coexistence        | Metadata | FIT download | Resume | Course/workout upload |
| -------------------------------------- | -------------------------- | -------- | ------------ | ------ | --------------------- |
| Garmin ID appears when phone is absent | Unsafe; active proof gated | Gated    | Gated        | Gated  | Gated                 |

## USB operations

| Hotplug             | Pairing marker | MTP enumerate                         | FIT import                                 | Reconnect/deduplicate | Course/workout write          |
| ------------------- | -------------- | ------------------------------------- | ------------------------------------------ | --------------------- | ----------------------------- |
| Attached state only | Unknown        | Raw and GVFS; 366-entry GVFS snapshot | Complete read/decode only; no import proof | Unknown               | Generic probe only; FIT gated |
