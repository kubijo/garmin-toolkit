# Connect IQ companion

## Scope and status

The [foreground probe](../../apps/garmin-connect-iq/source/ProbeApp.mc) samples battery, charging, daily steps, heart
rate and stress. It connects as a BLE central to a custom host peripheral; it does not use Garmin's private sync
protocol, a phone app, or HTTPS. Build targets are `fenix8solar47mm` and `edge1050`. The Edge 1050 on firmware 32.20 has
delivered battery/charging telemetry and resumed delivery after app relaunch on Linux 7.0.0-42 / BlueZ 5.85. The
progress display is physically verified as readable with stable layout. Missing steps, heart rate and stress remain
`null`; watch discovery and phone coexistence are unqualified.

The [Linux host](../../infra/python/connect_iq_host.py) registers a custom BlueZ GATT service and advertisement and
retains telemetry locally. Its [private-bus tests](../../infra/python/test_connect_iq_bluez.py) exercise real D-Bus
marshalling with fake BlueZ and device events; they never contact a system bus or radio. HASS ingestion is not
implemented. Physical qualification beyond the Edge observations above remains subject to
[ADR 0011](../decisions/0011-garmin-bluetooth-safety-gate.md). Earlier central-role discovery does not establish the
host's peripheral capability.

## Local tools

The pinned [Rafiki](https://bombsimon.github.io/monkey-c-rs/) formatter and linter run through `just qa::format` and
`just qa::preflight` without Garmin's SDK. It is also available in the development shell and as `nix run .#rafiki -- …`;
use `server` for its language server. SDK compilation remains the device compatibility check.

The local Zed extension in `infra/zed/monkey-c` supplies `.mc` highlighting, an outline, and the Rafiki LSP bridge.
Build its precompiled bundle with `nix run .#zed-monkey-c`; output is `dist/zed-monkey-c`. The bridge runs this
checkout's pinned `rafiki` flake app, so open the repository root in Zed and keep Nix available on its PATH. It provides
lint diagnostics, formatting and quick fixes; SDK navigation and completion are not included. Query checks run in
preflight. For extension development, `nix develop .#connect-iq-editor` supplies the Rust WASI target and grammar
compiler.

`nix run .#zed-monkey-c -- --install` also installs or updates this extension in Zed's local extensions directory; Zed
detects the files automatically. No marketplace publication or SDK is involved.

[ADR 0009](../decisions/0009-local-only-connect-iq-builds.md) keeps Garmin tools, keys and compiled outputs outside CI,
packages and releases. The [local runner](../../infra/python/connect_iq_setup.py) owns setup through the first build;
Garmin's agreement and account login remain owner-operated in SDK Manager.

Download SDK Manager from [Garmin's SDK page](https://developer.garmin.com/connect-iq/sdk/). Put its Linux ZIP at
`.tmp/connectiq-sdk-manager-linux.zip` or its macOS DMG at `.tmp/connectiq-sdk-manager.dmg`, then run:

```sh
just connect-iq::setup
```

In SDK Manager, sign in, install and activate an SDK, and install `fenix8solar47mm`. Close Manager to continue. Setup
discovers the selected SDK, creates or reuses a local RSA-4096 PKCS#8 DER signing key, checks prerequisites, records the
selected paths in `.tmp/connect-iq/local.json`, and compiles into a fresh local build directory. Subsequent builds use:

```sh
just connect-iq::build
```

Nix supplies Java 21, OpenSSL, Python and Linux Podman. The March 2026 Linux Manager requires WebKitGTK 4.0/libsoup 2.4,
removed from current nixpkgs. Manager and simulator use the
[owned compatibility image](../../infra/connect-iq/Dockerfile): an immutable Ubuntu 22.04 base plus a dated package
snapshot and Nix-provided CA roots. The image contains only public runtime dependencies; the owner-downloaded Manager is
mounted read-only when run. Container state stays under `.tmp`; SDK Manager uses the owner's normal Garmin data
directory. Linux Manager needs x86_64, rootless Podman and a desktop DISPLAY/XWayland session. The Linux runtime's
library resolution and GUI launch have been verified with the supplied ZIP.

On macOS, native `hdiutil` mounts the owner-supplied DMG read-only, `ditto` copies its app locally, and the image is
detached before launch. Native macOS acceptance remains outstanding; mount/copy cleanup is tested with fakes. Neither
platform bypasses OS approval prompts. Signing keys are never overwritten. Sideloading remains manual and subject to
physical acceptance authorization.

`--archive /path/to/download` selects another archive; `--sdk /path/to/sdk` reuses an installed SDK without Manager. The
lower-level `tools doctor`, `tools keygen`, `tools build`, and `tools prepare` commands remain available for
explicit-path diagnostics. `tools prepare` checks the Linux runtime without opening its GUI. Compiler output is retained
in each build's `build.log`. The sideloadable app is copied to `dist/connect-iq/probe.prg` after each successful build.
For the Edge 1050, run `just connect-iq::build --device edge1050`; its separate binary is
`dist/connect-iq/edge1050/probe.prg`. Select or tap the screen to start a scan. Both builds use the same BLE protocol
and on-device log filename; they must be installed on their matching device.

On Linux, `just connect-iq::test --device edge1050` runs owner-local SDK tests for the wire format, missing APIs, and
renderer bounds, then closes its simulator. Logs and structured coverage stay under `.tmp/connect-iq/test-*`.
`just connect-iq::preview --device edge1050` opens the real app for interactive inspection until the window closes,
Ctrl+C, or the one-hour limit; `--duration` selects a shorter limit. Both commands accept `fenix8solar47mm` and share an
exclusive simulator lock. Simulator checks cover runtime APIs and rendering, not physical BLE delivery.

SDK API availability can differ from firmware: the Edge 1050 on firmware 32.20 reports `Math has :min` as false, while
its API 6.0.3 simulator reports true. The progress renderer uses a comparison instead.

For on-watch diagnostics, create an empty `GARMIN/APPS/LOGS/probe.TXT` before launching the sideloaded `probe.prg`.
Garmin routes `System.println()` output to that file; reconnect USB after the run to retrieve it. The probe logs its
visible build identifier, millisecond timer, scan counts and service UUIDs, connection transitions, and GATT status
codes. It limits output to 501 lines per launch and omits sensor values, device names and addresses. Retrieve and clear
the log between experiments; it can accumulate across launches. Garmin's separate crash log is `CIQ_LOG.YML` or
`CIQ_LOG.BAK`, depending on the device.

An existing PKCS#1 DER key can be converted without changing its signing identity or replacing its source file:
`just connect-iq::tools keygen --key /path/to/new.der --from-key /path/to/existing.der`.

## Probe contract

The host advertises name `TK Probe` and service UUID `59102400-299d-4b88-a1ce-668c821d87ca`, with two characteristics:

- `59102401-299d-4b88-a1ce-668c821d87ca`: readable snapshot request.
- `59102402-299d-4b88-a1ce-668c821d87ca`: writable snapshot response, with ATT response.

A button press starts a ten-second scan. The app requires exactly one advertiser with the custom service UUID, connects,
then polls the request every five seconds while foregrounded. Connection allows sixty seconds for firmware confirmation;
GATT operations time out after ten seconds. The app releases only its own peripheral connection on stop; it does not
manipulate Garmin Connect pairing. The service name and UUID identify the probe; discovery does not require a name in
the same packet. Neither authenticates the peer.

Integers are little-endian. A four-byte request contains version `1`, operation `1`, and a `uint16` sequence. The
twenty-byte response has Python struct layout `<BBHIHBBIHH`: version, operation, sequence, Unix seconds, battery
percent, charging flag, reserved zero byte, steps, heart rate in bpm, and stress. Battery/heart-rate/stress use
`uint16`; steps use `uint32`. All-one bits mean unavailable, not zero. Charging is `0`, `1`, or unavailable `255`. The
host allows one outstanding request and retains its sequence once. Identical write retries are acknowledged without
duplicating a snapshot. Disconnect clears the pending request and peer lease. Correlation and peer isolation are not
device authentication or cryptographic replay protection.

Offline checks run with `python -m unittest discover -s infra/python -p 'test_connect_iq*.py'` in the pinned shell;
Linux wire tests use an isolated `dbus-daemon` and require no Garmin SDK.
`just connect-iq::tools inspect --response /path/to/frame.bin --sequence 0` decodes a captured response without radio
access.

## Linux host

For an approved physical run only:

```sh
just connect-iq::host --adapter hci0 --duration 300
```

The host requires an explicit adapter and runs for at most one hour. It registers only its own application and
advertisement; it never changes adapter power, alias, discoverability, bonding or trust, and never calls Garmin's native
protocol. BlueZ alone may deliver GATT callbacks. One device owns the request lease until disconnect. Rejected writes do
not consume requests, and retention failure returns an ATT error. BlueZ loss or adapter removal stops admission.
Shutdown unregisters owned resources and closes the dedicated bus connection, including after partial startup failure.

Captures use a fresh owner-private directory below `.tmp/connect-iq`. `telemetry.jsonl` records schema version `1`, host
receive time, and the decoded snapshot; missing measurements remain `null`. No device address is retained and no HASS
entities or events are published. Advertising capability and the host's BlueZ permissions must be qualified on the
actual host; the tool does not enable a controller or install system policy.

## Limits and extensions

Foreground telemetry is the first experiment. Runtime API availability and missing sensor values need device evidence;
there is no continuous background telemetry claim. Background scheduling and retained sensor histories require separate
qualification. The BLE characteristic API limits writes to twenty bytes, so larger histories need framing and flow
control.

Connect IQ exposes [recent sensor history][history] and [activity summaries][profile], not arbitrary original FIT files.
Daily monitoring is described by [ActivityMonitor][monitor]. HTTPS web requests are a separate route, constrained by
Garmin/phone certificate handling; self-signed HASS access must not be assumed to work.

Transport references: [central-role BLE][ble], [characteristic operations][characteristic].

[ble]: https://developer.garmin.com/connect-iq/api-docs/Toybox/BluetoothLowEnergy.html
[characteristic]: https://developer.garmin.com/connect-iq/api-docs/Toybox/BluetoothLowEnergy/Characteristic.html
[history]: https://developer.garmin.com/connect-iq/api-docs/Toybox/SensorHistory.html
[monitor]: https://developer.garmin.com/connect-iq/api-docs/Toybox/ActivityMonitor/Info.html
[profile]: https://developer.garmin.com/connect-iq/api-docs/Toybox/UserProfile.html
