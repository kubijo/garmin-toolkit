# Device capabilities

Support is keyed by device/firmware/host/adapter/operation; untested cells are unknown.

## Validation baseline

| Field            | Value                                                   |
| ---------------- | ------------------------------------------------------- |
| Device           | Garmin fēnix 8 Solar, 47 mm                             |
| Firmware         | Manifest `2244`; later GVFS inspection reported `22.44` |
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
| Bike portability  | Edge 850             | MTP file transfer      |
| Bike baseline     | Edge 1050            | MTP file transfer      |

Venu tests watch portability; Edge 850 extends the established Edge 1050 bike support. Record firmware, host, and
adapter for every run.

Simultaneous Edge 1050 and Venu 3S attachment was observed, but it does not establish complete explorer or write
acceptance for either device.

On 2026-11-03, the owner verified Edge 1050 discovery, directory browsing, activity and Course FIT previews, and
explicit activity import through the development HASS app on Raspberry Pi with a direct USB connection. Firmware was not
recorded for this run. After redeployment of the refresh fix, the owner confirmed that refreshing retains the navigated
directory. Cancelling the HASS upload picker also preserved the directory without an error or creating a file.
Cancelling removal confirmation retained the selected file and current directory.

Index S2 is available but deferred with other devices lacking USB data access. The initial device scope is USB watches
and bike computers.

Sources:
[Edge 850 computer connection](https://www8.garmin.com/manuals/webhelp/GUID-5BA20A50-BFFF-4418-AE4E-CA719C39EB05/EN-US/GUID-B4663AC8-EEF9-4684-883E-3CA79B0351EB.html),
[Edge 1050 file transfer](https://www8.garmin.com/manuals/webhelp/GUID-08ACA9FC-DEE6-4C8D-8A95-F62181C512E9/EN-US/GUID-50017E96-C40C-471D-BE61-3A0C09E93916.html),
[Index S2 setup](https://www8.garmin.com/manuals/webhelp/GUID-0BADEBCF-960D-4961-856C-86B9204BC169/EN-US/GUID-4477BB33-A532-4DE8-B8EC-DC28F6349AB4.html),
and [Index S2 specifications](https://www.garmin.com/en-GB/p/679362/pn/010-02294-70/).

## Bluetooth operations

| Operation                                             | Evidence                               |
| ----------------------------------------------------- | -------------------------------------- |
| Passive discovery                                     | Garmin ID appears when phone is absent |
| Pairing/coexistence                                   | Unsafe; active proof gated             |
| Metadata, FIT download, resume, course/workout upload | Gated                                  |

## USB operations

These rows describe recorded acceptance, not every operation exposed by the adapter or manifest.

| Device / path         | Verified operations                                                       |
| --------------------- | ------------------------------------------------------------------------- |
| fēnix / Linux GVFS    | Manifest, FIT reads/decode, probe create/read/delete, map maintenance     |
| fēnix / raw MTP       | Manifest and complete FIT reads/decode                                    |
| Edge 1050 / GVFS      | Map update/removal, restart acceptance, interrupted-update rollback       |
| Edge 1050 / HASS Pi   | Discovery, browse, activity/Course preview, activity import, path refresh |
| Edge 1050 / Linux x64 | AppImage discovery/reconnect, browse, FIT activity preview                |
| Edge 1050 / ARM64 VM  | Automatic mounting, discovery, browse, FIT activity preview with map      |
| Venu 3S               | Simultaneous attachment with Edge 1050 only                               |
| Edge 850              | No recorded physical acceptance run                                       |

Evidence: [desktop reconnect](usb-sync.md#physical-desktop-reconnect),
[physical pairing revisions/reassignment](usb-sync.md#physical-pairing-marker-acceptance),
[map maintenance](usb-sync.md#map-maintenance-evidence), and
[Course recovery/firmware discovery](usb-sync.md#course-transfer-evidence).

Raw-MTP limitations are recorded with [the adapter evidence](usb-sync.md#existing-adapters). Packaged desktop/ARM64
guest details are in [desktop packaging](desktop-packaging.md); a Linux VM on macOS does not qualify a native macOS
adapter. Outstanding checks live in [the work list](../plans/README.md#hardware-and-owner-access).
