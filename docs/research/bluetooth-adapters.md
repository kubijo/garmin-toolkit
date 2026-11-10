# Bluetooth adapters

## Result

The adapter layer is feasible; physical private-protocol experiments remain barred by ADR 0011. If cleared, use
[`bluer` 0.17.4](https://docs.rs/bluer/0.17.4/bluer/) (BSD-2-Clause) on HASS/Linux and evaluate
[`btleplug` 0.12.0](https://docs.rs/btleplug/0.12.0/btleplug/) (MIT/Apache-2.0/BSD-3-Clause) on macOS.

## Contract constraints

- Identity is opaque and adapter-scoped; MAC addresses are optional.
- Discovery is owned and bounded. Filters are hints followed by post-filtering because BlueZ merges client filters.
- One backend owns a physical adapter in a process. Do not hand the same controller between `bluer` and `btleplug`.
- OS bonding and Garmin pairing are separate. `btleplug` lacks bonding controls exposed by `bluer`.
- Share adapter state, discovery, connection, GATT I/O, notifications, cancellation, and classified diagnostics; keep
  backend handles/errors private.

## Identity smoke command

The opt-in `garmin-device/gfdi-smoke` feature composes `garmin-gfdi` with `bluer` on Linux. Ordinary discovery does not
invoke it. Run the complete driver with a synthetic peer and a new capture directory, without a system bus or radio:

```sh
just dev::gfdi-smoke simulate --capture-dir .tmp/gfdi-smoke-demo
```

Exercise the actual `bluer` backend against a private fake BlueZ service, including refused preconditions, identity,
timeout, and cleanup failures:

```sh
just dev::gfdi-smoke-check
```

This starts an isolated D-Bus daemon and a child command with its system-bus address redirected to that daemon. It does
not contact the host BlueZ service or a radio.

The native entrypoint is compiled but physically unqualified. Only after the separately approved ADR 0011 gate, select
the exact disposable device's already-existing host bond in `GFDI_TEST_ADDRESS`:

```sh
just dev::gfdi-smoke live --adapter hci0 --device "$GFDI_TEST_ADDRESS" \
    --capture-dir .tmp/gfdi-smoke-live --timeout-secs 30
```

The adapter must already be powered. The command refuses already-connected targets, does not scan or pair, and never
changes trust or adapter settings. It selects a known characteristic pair, requests one GFDI channel when needed,
receives a validated unsolicited identity, closes only its known handle, and disconnects. Default registration requests
MLR; `--plain` explicitly selects plain MultiLink with no automatic fallback. Authentication requests stop the run. No
GFDI host-information, configuration, time, file-sync, archive, or erase replies are sent. Devices requiring a fuller
handshake before identity will time out; do not infer compatibility from the synthetic peer.

The observation deadline is 1–60 seconds, including setup; cleanup allows two seconds for owned-handle closure and three
for disconnect. Timeout, Ctrl+C, capture failure, and malformed input all enter cleanup. Cleanup failure prevents a
successful result. BlueZ connections are shared: the initial connected-state check is not an atomic ownership lock, so
physical acceptance must exclude competing host clients and separately qualify phone coexistence.

Each fresh directory is owner-only (`0700`) with `events.jsonl` (`0600`), recording route, elapsed time, and raw sent
and received bytes. Captures are capped at 4096 events and 2 MiB of raw payload and may contain device identity.
Existing paths are never overwritten. Synthetic tests exercise deadlines, cancellation, transport faults, capture
exhaustion, and cleanup; the private-bus checks cover D-Bus marshalling. Neither qualifies a real BlueZ daemon or device
firmware.

## First pairing command

`pair` establishes the OS bond through BlueZ's default desktop agent, then exchanges Garmin host identity and minimal
capabilities and sends `PAIR_COMPLETE`, `SYNC_COMPLETE`, and `SETUP_WIZARD_COMPLETE`. It requires a matching sequenced
device acknowledgement for each outgoing setup command; an Ubuntu "paired" status alone is not success. This flow is
adapted from Gadgetbridge, but remains physically unqualified and subject to ADR 0011.

After selecting the target address in `GFDI_TEST_ADDRESS`, put the device into **Phone → Pair Smartphone** and keep
Ubuntu's Bluetooth settings open for code confirmation. The target must be known to BlueZ and disconnected; use the
settings window for discovery if needed, then `bluetoothctl devices` to obtain its address. Run:

```sh
just dev::gfdi-smoke pair --adapter hci0 --device "$GFDI_TEST_ADDRESS" \
    --capture-dir .tmp/gfdi-pair-live --timeout-secs 180
```

The command accepts an existing bond for setup retries. It never forgets the phone, removes a bond, changes trust, sets
time, downloads files, or claims phone coexistence. First pairing intentionally changes companion setup state; the
setup-complete events' effects on retained device data still require physical qualification. Auth negotiation echoes
Gadgetbridge's opaque flags; cryptographic Garmin authentication is unsupported and stops the run. Default mode is MLR;
`--plain` explicitly selects plain MultiLink. Unsequenced setup ACKs cannot complete the handshake.

Timeout, Ctrl+C and protocol failures enter owned-channel cleanup and disconnect. A failed handshake can leave the new
OS bond in place; the command does not undo pairing automatically. Private captures use the download limits below and
record the setup outcome. The private BlueZ tests exercise new and existing bonds, plain/MLR setup, rejected
confirmation, setup rejection, timeout and failed cleanup without contacting the host's Bluetooth service.

## FileAccess download command

The experimental Linux command lists without excluding synced items, finishes pagination, then pulls the first
`FIT_TYPE_4` item in listing order through a separate MultiLink transfer pipe. It uses Buffa-generated Protobuf types;
there is no legacy file-index fallback. Simulate the complete flow without a bus or radio:

```sh
just dev::gfdi-smoke simulate-download --capture-dir .tmp/gfdi-download-demo
```

Live use has the approval boundary and existing-bond/controller preconditions described above:

```sh
just dev::gfdi-smoke download --adapter hci0 --device "$GFDI_TEST_ADDRESS" \
    --capture-dir .tmp/gfdi-download-live --timeout-secs 300
```

`--plain` explicitly selects plain MultiLink; default mode is MLR, with no automatic downgrade. The download sends
host-information acknowledgement, listing/pull requests, transport/Protobuf acknowledgements, transfer-status responses,
and registration/cleanup for its known channels. It never pairs, authenticates, sends time/settings or a legacy sync
flush, or changes archive/synced flags. Phone coexistence and real firmware remain unqualified.

The Bluetooth session allows 1–600 seconds plus bounded cleanup, at most 32 listing pages/10,000 items, and one file up
to 16 MiB. It validates negotiated compression, complete zlib stream, exact uncompressed size, and activity FIT
integrity. After cleanup and disconnect, it atomically creates `activity.fit` (`0600`) inside the new private capture
directory. The retained event records its SHA-256, item UID, size, and device firmware/product. Existing artifacts are
never overwritten. Protocol, cleanup and validation failures retain private raw evidence without publishing an activity.
Restart with a new capture; partial transfer resume and database import are not implemented.

Download captures allow at most two million events and 64 MiB of raw protocol payload, unlike the smaller identity
capture. Both compressed input and decompressed output have bounds. Synthetic peers exercise complete plain/MLR
transfers, invalid results, corruption, cancellation, transport faults and cleanup. They establish no physical
compatibility.

## Evidence

An ignored Rust 1.98.0 spike built both crates. Each scanned the same Linux controller without connection; consecutive
use in one process exposed a BlueZ discovery-stop race, while isolated runs passed.

Linux desktop and Raspberry Pi HASS expose powered central-role BlueZ controllers. HASS has `bluetoothctl`, not
`busctl`; no discovery identifiers were retained.

The watch advertised nothing identifiable while phone Bluetooth was on. With it off, one unnamed, unpaired Garmin
manufacturer advertisement appeared; no connection followed.

Further proof needs a disposable/reset device and complete inventories. macOS waits for Garmin protocol safety.

## Gadgetbridge source basis

The [download plan](../plans/garmin-bluetooth-downloads.md) targets complete activity FIT files, separately from Connect
IQ telemetry. Its source reference is [Gadgetbridge revision `28cea544`][gb-audit]. The inspected transfer files and
this repository declare AGPL-3.0-or-later; adapted code must preserve upstream copyright and attribution.

- `GarminSupport.java` sends legacy `SetFileFlagsMessage(ARCHIVE)` after download and when skipping an existing local
  file. Newer downloads call `FileSyncServiceHandler.markSynced()`, which emits `ModifyFlagsRequest`. Both are gated by
  `getKeepActivityDataOnDevice()`, whose default is false. These are device-state changes, not transport
  acknowledgements.
- `FileTransferHandler.java` starts legacy directory downloads and selects the new-file filter after synchronization.
  `FileSyncServiceHandler.kt` excludes already-synced items by default. Backfill must deliberately handle both filters.
- The newer file listing maintains cursor/session and transaction IDs. File pulls request the MultiLink transport pipe
  and compression. `GarminSupport.onFetchRecordedData()` normally starts a legacy download to flush data even for the
  newer protocol; an option bypasses it because some firmware freezes on that request.
- `CobsCoDecTest.java` supplies framing, chunking, truncation, resynchronization, and overflow cases. It is useful
  conformance input, not evidence that our transfer implementation or the Edge 1050 firmware is qualified.
- `GarminEdge1050Coordinator.java` supplies model matching and a battery-reporting exception, not a model-specific
  file-transfer implementation. Shared code alone does not establish the correct route on firmware 32.20.
- `CommunicatorV2.initializeDevice()` sends `CLOSE_ALL` for Gadgetbridge's client ID before registering GFDI. Our
  offline negotiator instead returns a single explicit `REGISTER_ML_REQ` for service 1 and a caller-selected client ID.
  It does not inherit close-all or automatic re-registration. Allocating a handle still changes peer session state; it
  is not proof of a non-mutating connection or permission to use a data-bearing device.

Do not inherit the Android coordinator's default sync behavior. Distinguish persistence flags, sync flushes, OS bonds,
Garmin registration, and transport flow control before any physical run. No upstream code was executed on a device.

AGPL-3.0-only
[garmin-bridge 0.1.0](https://github.com/wh1le/garmin-bridge/commit/5b849c7b81c005765560b7193d28c580ac80ca2c) uses BlueZ
but does not prove coexistence or history sync. Neither is a dependency or a substitute for the safety gate.

## Additional protocol reference

[braycarlson/gfdi at `a7d92daf`][zig-gfdi] is a Zig FIT-download implementation declaring MIT licensing. Useful
comparison points are `src/protocol/{gfdi,file_sync}.zig`, `src/session/session.zig`, fake transports, and fuzz cases.
Its legacy directory records agree with Gadgetbridge's sixteen-byte layout. Retain Gadgetbridge as the primary source
for MLR: the Zig tool requests plain MultiLink and sends fragments with a single handle byte.

Its session replies to time requests, advertises host information, and sends `SYNC_READY`; those actions are outside our
identity probe. Its legacy history fallback probes indices 1–1024 and stops after sixteen refusals, which cannot
establish complete historical listing. The label “archive sweep” means probing old indices, not evidence of a file-flag
mutation. None of these policies should be inherited as defaults or treated as firmware qualification.

[gb-audit]: https://codeberg.org/Freeyourgadget/Gadgetbridge/commit/28cea5443cfa35ff1dfe047645e7abf18ced24cc
[zig-gfdi]: https://github.com/braycarlson/gfdi/tree/a7d92dafb75cfb5d66577f8015fae3f7dfce7c67
