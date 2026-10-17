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

All four devices are available and support Bluetooth and Wi-Fi. fēnix/Venu are watches; Edge models are bike computers.

| Role              | Exact device         | Documented USB path    |
| ----------------- | -------------------- | ---------------------- |
| Baseline          | fēnix 8 Solar, 47 mm | MTP/Garmin USB mode    |
| Watch conformance | Venu 3S              | Verify mode physically |
| Second family     | Edge 850             | MTP file transfer      |
| Bike conformance  | Edge 1050            | MTP file transfer      |

Venu tests watch portability; Edge 850 establishes bike support and Edge 1050 tests conformance. Record firmware and
host for every run.

Simultaneous Edge 1050 and Venu 3S attachment was observed, but it does not establish complete explorer or write
acceptance for either device.

Index S2 is available but deferred with other devices lacking USB data access. The initial device scope is USB watches
and bike computers.

Sources:
[Edge 850 computer connection](https://www8.garmin.com/manuals/webhelp/GUID-5BA20A50-BFFF-4418-AE4E-CA719C39EB05/EN-US/GUID-B4663AC8-EEF9-4684-883E-3CA79B0351EB.html),
[Edge 1050 file transfer](https://www8.garmin.com/manuals/webhelp/GUID-08ACA9FC-DEE6-4C8D-8A95-F62181C512E9/EN-US/GUID-50017E96-C40C-471D-BE61-3A0C09E93916.html),
[Index S2 setup](https://www8.garmin.com/manuals/webhelp/GUID-0BADEBCF-960D-4961-856C-86B9204BC169/EN-US/GUID-4477BB33-A532-4DE8-B8EC-DC28F6349AB4.html),
and [Index S2 specifications](https://www.garmin.com/en-GB/p/679362/pn/010-02294-70/).

## Operations

| Operation                                             | Evidence                               |
| ----------------------------------------------------- | -------------------------------------- |
| Passive discovery                                     | Garmin ID appears when phone is absent |
| Pairing/coexistence                                   | Unsafe; active proof gated             |
| Metadata, FIT download, resume, course/workout upload | Gated                                  |

## USB operations

| Operation             | Evidence                                   |
| --------------------- | ------------------------------------------ |
| Hotplug               | Attached state only                        |
| Pairing marker        | Unknown                                    |
| MTP enumerate         | Raw and GVFS; 366-entry GVFS snapshot      |
| FIT import            | Complete read/decode only; no import proof |
| Reconnect/deduplicate | Unknown                                    |
| Course/workout write  | Generic probe only; FIT gated              |
