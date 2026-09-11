# USB synchronization

## Direction

Before consent, inspect only attachment descriptors. After consent, stage, hash, and atomically commit originals with
disconnect recovery. Writing is separate; reading never deletes.

HASS may request raw USB/udev. Current fēnix/Edge devices expose MTP; mass storage remains a separate adapter.

Sources: [HASS app USB/udev configuration](https://developers.home-assistant.io/docs/apps/configuration/),
[fēnix 8 USB modes](https://www8.garmin.com/manuals/webhelp/GUID-EECCAC99-90D6-4AB1-9A3A-EC433D3365E2/EN-US/GUID-B6EEC065-0BAB-4A19-8350-A3A9DA44AD1D.html),
and
[Edge 1050 file transfer](https://www8.garmin.com/manuals/webhelp/GUID-08ACA9FC-DEE6-4C8D-8A95-F62181C512E9/EN-US/GUID-50017E96-C40C-471D-BE61-3A0C09E93916.html).

## Official storage contract

Garmin's [`GarminDevice` v2 XSD](https://www8.garmin.com/xmlschemas/GarminDevicev2.xsd) defines per-device file shapes,
locations, and `InputToUnit`, `OutputFromUnit`, or `InputOutput` direction.

The fēnix manifest at `GARMIN/GarminDevice.xml` reports `fenix 8 - 47mm, Solar`, software `2244`, and these FIT paths:

| Manifest type        | Output from watch  | Input to watch    |
| -------------------- | ------------------ | ----------------- |
| Activity             | `Garmin/Activity`  | `Garmin/NewFiles` |
| Workout              | `Garmin/Workouts`  | `Garmin/NewFiles` |
| Course               | `Garmin/Courses`   | `Garmin/NewFiles` |
| Schedule             | `Garmin/Schedule`  | `Garmin/NewFiles` |
| Location             | `Garmin/Location`  | `Garmin/NewFiles` |
| Monitor              | `Garmin/Monitor`   | —                 |
| Metrics              | `Garmin/Metrics`   | `Garmin/NewFiles` |
| Sleep                | `Garmin/Sleep`     | `Garmin/NewFiles` |
| Summary              | `Garmin/SUMMARY`   | `Garmin/NewFiles` |
| HRV status           | `Garmin/HRVStatus` | `Garmin/NewFiles` |
| Skin temperature     | `Garmin/SkinTemp`  | —                 |
| Totals               | `Garmin/Totals`    | —                 |
| Records              | `Garmin/Records`   | `Garmin/NewFiles` |
| Device configuration | `Garmin/Device`    | —                 |

It also declares settings, sports, goals, golf, dive, calendars, backups, maps, media, Wi-Fi, Connect IQ, debug, and
updates. Device ID and update inventory were not retained.

Garmin documents [`NewFiles`](https://support.garmin.com/en-MY/?faq=rzvP53Si4O3barYoXzw5L7) as post-disconnect input,
[`Garmin/Activity`](https://support.garmin.com/en-CA/?faq=Ht3ZP52Kju075uKvqTqu99) as recorded output, and
[FIT types](https://developer.garmin.com/fit/file-types) independently of folders.

Under [ADR 0012](../decisions/0012-manifest-driven-usb-capabilities.md), output permits copy, input only identifies a
potential operation, and unlisted paths grant nothing.

## Device identity

[ADR 0018](../decisions/0018-on-device-profile-marker.md) makes a root TOML marker the sole persisted association. It
contains immutable user/device UUIDs and mutable profile context while preserving comments and unknown fields.

Manifest, USB, MTP, and FIT IDs are diagnostic; none identifies an application profile.

## Existing adapters

`garmin-device` owns attachments, consented discovery, capabilities, and I/O; apps see no backend handles or raw paths.

| Candidate                                                                                                  | Treatment                                                                                                                                                                                                                                        |
| ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [`nusb` 0.2.7](https://github.com/kevinmehall/nusb/commit/bdc148c123c102785cd1d506b77bfeeb794ffeb1)        | Primary hotplug and descriptor candidate. It is pure Rust, MIT/Apache-2.0, runtime-neutral, and supports Linux, macOS, and Windows.                                                                                                              |
| [`mtp-rs` 0.32.0](https://github.com/vdavid/mtp-rs/commit/4069f1f4c424b38e471f44c99de8462a25ca13ba)        | Primary MTP candidate. It is runtime-neutral, MIT/Apache-2.0, uses `nusb` on Linux/macOS and Windows WPD behind one high-level API, and has mock and virtual-device tests. Upstream reports a Forerunner 955 and Venu 2/2S, but not the fēnix 8. |
| [`libmtp`](https://github.com/libmtp/libmtp/commit/eb12290bdde39c59d709f824389837cbfb63ab15)               | Diagnostic and fallback reference only. Its C/FFI stack and LGPL-2.0-or-later obligations add packaging work; activate it only for an owned-device failure tied to a quirk `mtp-rs` cannot implement safely.                                     |
| [`libmtp-rs` 0.7.7](https://github.com/quebin31/libmtp-rs/commit/002b8080dff2e95ce66ae331780fa32a38842dd3) | Reject. The MIT wrapper is stale alpha software, lacks partial transfers and events, and still requires system libmtp through `pkg-config`.                                                                                                      |

`mtp-rs` recognizes Garmin's vendor interface and split transfers; failed uploads expose partial objects without
deleting them. Its 0.32.0 split-header streaming branch sends every payload chunk with `send_bulk`, bypassing the USB
streaming path that terminates packet-aligned payloads with a zero-length packet. A raw 256 MB fēnix probe consequently
sent every byte and then timed out waiting for finalization. The adapter now reserves the final source byte for a short
USB transfer without changing the object size or content; a regression test covers packet-aligned payloads. Hardware
confirmation of that correction remains part of the production proof below.

libmtp names Venu 3S and Edge 850, not fēnix `091e:51b4`, and assigns Android/reset quirks. This is diagnostic evidence,
not fallback. libmtp remains LGPL-2.0-or-later regardless of wrappers.

The fēnix raw path later timed out after sending `OpenSession`; the device returned no bulk response. `mtp-rs`'s
session-less reset then sent USB Still Image Class `DEVICE_RESET` (`0x66`), which this vendor-class Garmin interface
rejected with a control-endpoint stall. That is not the recovery used by libmtp. Its established failed-`OpenSession`
path performs a whole USB port reset before reopening, and its quirk table gives every listed modern Garmin the
`FORCE_RESET_ON_CLOSE` flag through `DEVICE_FLAGS_ANDROID_BUGS`. `nusb::Device::reset` exposes the equivalent
whole-device operation without adding libmtp. The adapter now uses that operation only after a bounded open timeout,
only for the exact selected location in the known Garmin VID/PID set, waits quietly, and retries inspection once. It
does not reset healthy sessions.

This is an established recovery attempt, not a known universal fix. Capture `.tmp/fenix-raw-link-05` confirmed that the
whole-device reset completed on the retained fēnix, followed by the quiet period, but the next `OpenSession` again
received no response and timed out. The bounded transport sequence itself took about 25 seconds: one 10-second open,
reset plus five seconds quiet, and one 10-second retry. A public Epix Pro report records the same outcome, and no public
end-to-end resolution for the fēnix 8 Solar identity `091e:51b4` was found. The fēnix 8 manual exposes a separate Garmin
USB mode. One fēnix 8 owner reports that selecting it restored Garmin Express connectivity on Windows after firmware
20.19, but that demonstrates Garmin's proprietary host path rather than a raw-MTP fix. The research found no published
or open implementation that uses Garmin mode for arbitrary map-file transfer; Wi-Fi Map Manager is likewise a
device-side facility rather than a documented host transfer API.

Consequently, automatic reset is disabled for `091e:51b4`, and an unresponsive instance now fails after the bounded open
with a physical-reconnect instruction. After a fresh connection, raw link benchmarking retains one caller-owned MTP
session across manifest inspection, capacity validation, upload, read-back, and cleanup. This follows libmtp's stronger
recommendation for responders that break under command-line-style repeated session open/close cycles.

Sources: [libmtp failed-open recovery](https://github.com/libmtp/libmtp/blob/master/src/libusb-glue.c#L1918-L1935),
[libmtp Garmin quirks](https://github.com/libmtp/libmtp/blob/master/src/music-players.h#L4069-L4113),
[libmtp Android/reset flags](https://github.com/libmtp/libmtp/blob/master/src/device-flags.h#L308-L320), and
[`nusb::Device::reset`](https://docs.rs/nusb/0.2.7/nusb/struct.Device.html#method.reset). See also the
[failed Epix Pro reset](https://github.com/libmtp/libmtp/issues/245) and Garmin's
[fēnix 8 USB-mode documentation](https://www8.garmin.com/manuals/webhelp/GUID-EECCAC99-90D6-4AB1-9A3A-EC433D3365E2/EN-GB/GUID-B6EEC065-0BAB-4A19-8350-A3A9DA44AD1D.html)
and a
[fēnix 8 Garmin-mode recovery report](https://forums.garmin.com/outdoor-recreation/outdoor-recreation/f/fenix-8-series/425739/fenix-8-47-amoled-software-20-19/1989239).

An ignored Rust 1.98.0 checkout of `mtp-rs` 0.32.0 passed 408 virtual-device tests; six hardware tests were ignored.
This does not prove Garmin compatibility.

Keep `mtp-rs` types private; expose only owned identity, lifecycle, capability, I/O, and diagnostic types.

HASS/Linux use `nusb` and `mtp-rs`; Windows uses WPD. macOS remains unproven because `ptpcamerad` may own the device.
Mass storage may use an existing mount but never mounts block devices.

## Observed desktop attachment

On Linux, fēnix USB `091e:51b4` exposed one MTP vendor interface, `ID_MTP_DEVICE=1`, and no block device. GVFS mounted
it before consent.

Switching the same watch to Garmin USB mode exposed `091e:0003`: one USB 2.0 High Speed vendor interface with bulk
input, bulk output, and interrupt input endpoints. It exposed neither MTP nor mass storage. This identity uses Garmin's
record protocol and must not enter the MTP allowlist. Garmin documents that protocol for waypoints, routes, and track
logs, not arbitrary files or maps. A read-only protocol inventory still needs device-node access.

The watch can update supported maps itself through Map Manager over Wi-Fi, but Garmin requires Wi-Fi plus external power
for that workflow. Garmin Express inspection found host-side HTTP downloads followed by device writes; its Wi-Fi code
configures networks and does not provide a map-transfer data plane. No documented host API was found for controlling the
firmware workflow, and Connect IQ downloads remain app-scoped. Map Manager has the same long-running, sleep-sensitive
device behavior this toolkit is intended to replace, so it is only a user-operated fallback and not a production
transport adapter. Sources:
[fēnix Map Manager manual](https://www8.garmin.com/manuals/webhelp/GUID-EECCAC99-90D6-4AB1-9A3A-EC433D3365E2/EN-GB/GUID-4B36978F-C23E-45D5-8F72-011965DA056C.html),
[Garmin map download requirements](https://support.garmin.com/en-GB/?faq=VFHzF1UDEN6UWekbJid8W8).

After consent, GIO reported one writable `mtpfs`: 31,058,427,904 bytes total and 17,426,481,152 free. No locator,
payload, serial, or ID was retained; no transfer occurred.

A second consent found 100 directories, 266 files, 185 FIT objects, and 265 files under `GARMIN`, including activity,
health, course, workout, and intake paths. Filenames were not retained.

A 42-byte root probe was created, verified, deleted, and confirmed absent with unchanged counts.

A third consent released the Garmin GVFS mount and opened raw `mtp-rs`. The read-only check parsed `GarminDevice.xml`,
followed its activity capability, read the two newest complete FIT objects, verified their MTP sizes, and decoded one
run and one ride. No private data or identifiers were committed.

The adapters separate discovery from reads, share manifest parsing, hide locations, and stream into caller staging. GIO
repeated the two-file read while GVFS owned the watch. Raw MTP reported `DeviceBusy`, then repeated the read after
explicit release. Tests cover capability filtering, size, cancellation, unrelated files, unsafe paths, and source
changes.

This read-access baseline proves attachment identity, enumeration, manifest discovery, raw and mounted complete FIT
reads, decoding, initial watch delivery, and generic GVFS create/read/delete. It does not prove later hotplug,
reconnect, HASS permissions, or device acceptance of written Garmin content.

## Map-maintenance evidence

User-reported CLI runs separately showed successful map download, verification, and authorization capture on fēnix 8
Solar and Edge 1050 without device commit. An adjacent attachment inventory identified the desktop-mounted device as
`Fenix 8 - 47mm`, USB `091e:51b4`. Its 256 MB disposable mounted-MTP benchmark uploaded at 15.83 MB/s in 16.17 seconds,
received final acknowledgement in 0.03 seconds, read the object back at 2.67 MB/s in 95.96 seconds with a verified
SHA-256 digest, and removed it in 0.79 seconds. This proves the desktop-mounted whole-object path for that session, not
the raw-MTP path or firmware acceptance of map content.

At revision `e981670`, the retained fēnix `t03` transaction recovered without device-file read-back. It accepted all 18
writes by path and size, completed the remaining 6 of 9 removals, and finished with 11.07 GB free. The 16.92 GB retained
payload set verified in 43 seconds and device reconciliation took 30 seconds.

Edge 1050 (`006-B4440-00`) then completed two guarded removal runs. The first removed 9 files from TopoActive Middle
East & Central Asia, North Africa, and South Africa and reclaimed 8.42 GB. The second removed the 2.54 GB TopoActive
Australia & Oceania component. Verified backups were retained. A subsequent backup-free update reused and verified 15
cached files, wrote 18.65 GB for Trailforks and TopoActive Central, Eastern, and Western Europe in 17 minutes 12
seconds, and finished with 39.82 GB free of 61.89 GB. All uploaded paths and sizes passed inspection.

The same Edge session proved that an attempt cancelled before portable publication can be discarded without invoking
recovery. On 2026-09-11, no-capture inspections after safe restarts found fēnix firmware 22.44 and Edge firmware 32.20
through their GVFS mounts. Neither device offered recovery. The fēnix inventory retained its recovered 2026.11 maps; the
Edge inventory retained Trailforks and the three updated 2026.11 European components, while the previously removed
components remained available only as installs. This is black-box restart acceptance of both completed transactions.

The Edge then installed the disposable 946.08 MB TopoActive Antarctica component with verified recovery backups. After a
safe restart, the firmware inventory exposed Antarctica as installed. A normal removal backed up and verified all four
component files, reclaimed the space, and completed in 58 seconds. A cached reinstall was then killed with `SIGKILL`
during commit, after 148.39 MB of the first 222.96 MB map file had been written. On the next production TUI start, the
portable transaction blocked further changes and offered recovery. Verified-backup recovery rolled the mounted-MTP
update back, reconciled device state, and restored 39.82 GB free. After another safe eject, restart, and remount, the
TUI presented no recovery prompt and offered Antarctica only as an install. This completes the current fēnix and Edge
black-box update, restart, and interrupted-recovery matrix. Private captures are not publication artifacts.
