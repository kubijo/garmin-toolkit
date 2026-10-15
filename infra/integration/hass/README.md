# HASS integration tests

## Home Assistant app package

The `garmin-hass-app-image` flake output is defined to package the production host and browser assets as an
OCI-compatible Docker image for the builder's Linux architecture.
[App metadata](../../../apps/garmin-hass/addon/config.yaml) declares AMD64 and ARM64, raw USB access, ingress on port
8099, and a cold backup of persistent `/data`. The image selects raw MTP directly and accepts HTTP and WebSocket
connections only from the Supervisor ingress gateway. The ordinary standalone production host retains its
desktop-mounted MTP source.

For standalone hosting, `GARMIN_TOOLKIT_HASS_ADDRESS` overrides `127.0.0.1:8099`, and `GARMIN_TOOLKIT_HASS_WEB_ROOT`
overrides the package's bundled frontend default. A missing configured frontend fails startup.
`GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY` restricts requests to that socket peer when set; invalid addresses fail startup.
The host's startup tests run the real HTTP server with a fake device source in isolated subprocesses, covering custom
data/frontend roots, listener overrides, and allowed/rejected ingress peers in production and demo builds. Full QA and
CI include both configurations.

An image archive built on each architecture must be available through a registry before Supervisor can install this app.
The host-native runner below serves the locally built image from a registry inside its disposable guest. The ARM64 boot
smoke does not install an app; a native ARM CI job is configured to build and inspect that image. The CI job keeps the
ARM image as a one-day artifact for installation testing on an ARM64 HAOS device.

On 2026-10-14, the AMD64 image built successfully under a four-CPU, six-GiB Nix daemon cap. Its archive declares the
expected entrypoint, raw-MTP mode, ingress gateway, and persistent `/data`. A disposable Podman run served the packaged
UI with HTTP 200 when loopback was configured as the gateway; the image's default gateway setting rejected a loopback
request with HTTP 403. This validates image startup and the gateway restriction, not Supervisor installation. This
builder has no ARM64 Nix builder or binfmt handler; CI uses a native ARM runner for that image. QEMU can emulate a
directory-backed MTP USB device, but its fixed QEMU vendor ID (`0x46f4`) differs from the Garmin ID (`0x091e`) required
by production discovery. It cannot exercise the full Garmin attachment path without a controlled VID-capable emulator or
a passed-through device.

## Interactive HAOS app session

Use the app archive matching this host's CPU architecture:

```bash
just hass::haos-app-run --app-archive /nix/store/...-garmin-toolkit-hass.tar.gz
just hass::haos-app-stop SESSION_ID
```

The runner defaults to the host architecture (`--arch auto`), validates the app image before boot, and selects the
matching pinned HAOS image, QEMU, and firmware. It uses KVM when accessible for a native guest, with TCG as fallback.
`--arch amd64` and `--arch aarch64` select an explicit guest. The existing smoke command accepts the same selector.

The interactive command verifies an isolated headless Chrome page and screenshot capture before booting. It waits for
full Home Assistant Core, creates a disposable `admin` / `admin` account, installs the app, and verifies authenticated
ingress in that headless browser. Captures default to dark mode for Home Assistant and the embedded app. Once
verification passes, it prints the URL and credentials for manual inspection. `--no-verify-browser` skips the browser
check. The foreground runner owns the session until explicit stop, interruption, failure, or its two-hour inspection
timeout. Startup has a separate thirty-minute limit; `--session-timeout` can shorten inspection. `--port` chooses the
loopback HTTP port. An owned SSH tunnel forwards that port to the HTTP port reported by Core, so it also supports guests
using port 80.

The VM has four CPUs, four GiB of guest RAM, a verified six-GiB cgroup limit, and a sixteen-GiB overlay file-size limit.
Guest disks, temporary SSH identity, credentials, and browser profile live under repository `.tmp`; cleanup removes them
and the owned listeners. Only small version metadata, browser diagnostics, and screenshots remain under
`.tmp/haos-evidence`. The production app disables semantic automation and screenshot routes, so the maintained browser
helper verifies its canvas and Remoc connection through ingress and captures the app iframe using Chrome DevTools. The
configured MCP server cannot attach to that isolated browser; the helper uses its session-owned DevTools socket.

## Populated HAOS workflow

For a populated workflow through authenticated ingress, run:

```bash
just hass::haos-app-check --app-archive /nix/store/...-garmin-toolkit-hass.tar.gz
```

This creates a disposable profile, imports the project-owned
[GPX candidate fixture](../../../crates/garmin-gpx/tests/fixtures/candidates.gpx), generates one FIT Course, and
downloads both artifacts through ingress. It repeats those downloads after a Supervisor app restart and cold
backup/restore. After backup it deletes the Course and creates another profile; restore must recover the original Course
and remove that later profile. An independent SQLite oracle copies selected storage while Supervisor has the app
stopped, then reads the private database and WAL before reopening the app. Downloaded bytes must equal the stored
artifacts and the GPX must equal the imported fixture.

The test first verifies the unmodified production frontend. Its isolated browser then enables the semantic driver
already present in the web bundle by changing only that browser's canvas attribute before startup. Actions and app
screenshots use the built-in APIs; Chrome DevTools handles the browser-native file chooser and downloads. The production
host's HTTP control API remains disabled. The command exits and removes its VM, browser, listeners, credentials, and
disks after success or failure; screenshots, synthetic downloads, version metadata, and the result remain under
`.tmp/haos-evidence`.

Passed locally on 2026-10-15 with the AMD64 `0.1.0` image, HAOS 18.3, and Home Assistant Core 2026.10.1.

## HAOS app smoke

Build `.#packages.x86_64-linux.garmin-hass-app-image` with a memory-capped Nix builder, then run:

```bash
just hass::haos-app-smoke --app-archive /nix/store/...-garmin-toolkit-hass.tar.gz --timeout 1200
```

The runner verifies the pinned official HAOS 18.3 OVA, boots it with four CPUs and a hard six-GiB memory cap, loads the
archive, and serves it from a registry inside the guest. Only the guest's temporary local-app metadata points to that
registry; the source app metadata keeps its publication image name. Supervisor installs and starts the app. The runner
checks HTTP 200 from the Supervisor gateway, HTTP 403 from a second container on the same network, and no published host
port. It then selects a non-default storage generation, proves the app opens it with the original database temporarily
absent, creates a cold partial Supervisor backup, switches back to the original, restores the backup, and verifies the
selected generation and running app. The guest is isolated from the user's Home Assistant and devices; the disk-backed
overlay lives only under the repository's ignored `.tmp` and is removed with the VM. The overlay has an eight-GiB
file-size limit, including while Docker loads and pushes the image.

This passed locally on 2026-10-14 with the AMD64 `0.1.0` image. The guest uses the Home Assistant landing page instead
of an onboarded Core instance, so this tests the gateway rule and Supervisor lifecycle, not an authenticated frontend
ingress session. Registry publication and the ARM64 app install remain separate acceptance checks.

## ARM64 HAOS boot smoke

Run a fresh, networkless Home Assistant OS ARM64 guest on an x86 Linux host with QEMU software emulation:

```bash
just hass::haos-arm64-smoke
```

The runner downloads the pinned
[official HAOS 18.3 generic AArch64 QCOW2 image](https://github.com/home-assistant/operating-system/releases/tag/18.3)
to the repository's ignored, disk-backed `.tmp`, checks its published SHA-256, and boots a copy-on-write overlay. Pass
`--image /path/to/haos_generic-aarch64-18.3.qcow2.xz` to reuse a downloaded archive. It asserts that GRUB boots Slot A
and HAOS mounts its data partition. The VM has no network or USB access; its user scope is checked for a four-CPU and
six-GiB cap, its overlay is limited to two GiB, and all scratch files and VM processes are removed afterward. This
confirms that the ARM64 HAOS image boots in the disposable harness. It does not test Supervisor, app installation,
ingress, backup/restore, or Garmin hardware.

## NixOS application integration

Run on a Linux Nix builder with KVM:

```bash
just hass::integration
```

For the socket/reconnect diagnostic matrix:

```bash
just hass::integration --suite reconnect
```

This exercises both bare WebSocket APIs and application reconnects with WebSocketStream available or absent, fault hooks
enabled or disabled, and tracing enabled or disabled. Each of the 16 cases alternates eight abrupt/graceful host
restarts, then verifies process and state cleanup. Constructor counters assert that the application always selects
standard WebSocket. It is an explicit diagnostic target, separate from the route workflow check in CI.

The application explicitly selects `websocket-web`'s standard interface. On 2026-10-07 its automatic WebSocketStream
selection reproduced Chromium 153.0.8010.12's native null-pointer crash during reconnect, including without fault hooks
or tracing. The native stack matched the queued-restore failure; standard WebSocket passed 64 comparison reconnects.
Bare sockets also passed, so the evidence identifies the application/stream-transport interaction, not a specific
Chromium function. Keep the normal library buffering and transport semantics when changing this selection.

Both suites build the packaged demo and run a disposable NixOS VM with two CPUs and 4 GiB RAM. The route suite is also a
Linux flake check, so CI's `nix flake check` runs it; ordinary `qa::full` does not boot a VM. The local recipe runs the
Nix-built driver as your user so it can use your KVM access; CI runs it in the Nix builder. Both require hardware
acceleration and refuse to fall back to CPU emulation. The local Python runner uses Tyro and Rich for its CLI and
progress output. Build and VM output go to separate log files in the printed diagnostics directory; cancellation stops
the owned process group before removing temporary VM state. New files must be Git-tracked for the repository's
Git-backed flake to include them.

Nix pins the host, Playwright, and its matching Chromium. No npm cache, developer browser profile, pre-existing HASS
instance, or downloaded test data is used. Chromium runs headed on the VM's X display using Mesa's CPU renderer through
ANGLE GL, with software WebGL2 explicitly enabled. The application's map worker remains enabled; startup, reload, and
teardown assert renderer readiness. The browser cannot fetch external resources, and the VM has no external network.
Tests cover route geometry and workflows; they do not establish live map-provider availability or GPU-driver
compatibility.

Each test owns fresh demo storage, a host process group, and a browser. Test hooks close them and remove mutable state,
including after assertion failures. Hooks verify that owned process groups are gone. Systemd bounds the run and kills
its entire control group on timeout. The VM driver stops both test services and shuts down the VM in `finally`; its own
cleanup releases QEMU on driver failure. An intentional-failure probe checks that cleanup actually occurs and fails the
VM check if it does not.

The suite asserts:

- Demo seeds, reviewed GPX import, byte-exact source/Course downloads, deletion/cancellation, and versioned
  regeneration.
- Reviewed FIT Course transfer to the demo watch, live byte-verification status, exact device bytes, unchanged pairing
  marker, host restart reconciliation, and duplicate-free repeat send. Firmware discovery requires a disconnected
  device.
- Separate candidates, rejected siblings, unresolved controls, malformed input, and subsequent valid import.
- Profile isolation and upload interruption at an observed file-read boundary.
- Lost responses after independently observed database commits, with duplicate-free import and Course retries.
- Restore from a second client while old confirmation, generation, or download requests are queued.
- Restore from a second client across FIT encoding and after a client has received the first chunk of a multi-chunk
  Course download. The generation test holds a gate before and after the synchronous encoder call, with the same restore
  attempt pending at both boundaries. The tests check restored storage and exact FIT bytes. Cancelling a body before its
  first chunk must release restore without a test-side gate release.
- Graceful and abrupt host restarts, selected restored storage, and duplicate-free seeding.

Faults are installed only in cases that need them. They gate both WebSocket APIs and file reads; assertions establish
the pending boundary before switching profiles, disconnecting, or restoring. The storage oracle opens the selected
database read-only. Routes used for upload are downloaded from the real seeded recording through the product UI.

The checkpoints are enabled only in the automation demo host and are armed by files under each case's disposable data
root. Restore signals only after a nonblocking attempt finds the exclusive deployment lock held. The tests then assert
that restore remains pending and storage remains selected while the controlled operation is paused. Only gated demo
downloads split the recorded Course into 4 KiB HTTP chunks; normal downloads keep their usual chunking. The production
host has no checkpoints.

TAP output, host/browser logs, screenshots, and Playwright traces are retained under the check output's `artifacts`
directory, or the local recipe's printed `.tmp/hass-integration.*/results` directory. For failed Nix builds, pass
`--keep-failed` to retain the build directory and inspect `nix log`; its driver output contains the collected artifacts.
State is disposable; diagnostics are retained. On 2026-10-08 the packaged route suite passed all 14 cases and the
intentional-failure cleanup probe with standard WebSocket selected by the application.
