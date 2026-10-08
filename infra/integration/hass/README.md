# HASS integration tests

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
State is disposable; diagnostics are retained. On 2026-10-08 the packaged route suite passed all 13 cases and the
intentional-failure cleanup probe with standard WebSocket selected by the application.
