# HASS integration tests

## Repository-install acceptance

Pass a [generated store tree](../../release/README.md#generated-home-assistant-repository) and matching image to the
disposable runner:

```sh
just hass::haos-app-check \
    --app-archive /nix/store/IMAGE-garmin-toolkit-hass.oci.tar \
    --repository /nix/store/REPOSITORY-garmin-hass-app-repository \
    --timeout 1800
```

Replace the placeholders with actual paths. Starting with fresh HAOS and empty app state, the runner serves a temporary
`hass-apps` Git branch and guest registry. Only the copied metadata's image repository changes. The Git listener uses
host loopback, reachable from QEMU at `10.0.2.2`, with no LAN exposure.

The runner pushes OCI through an owned SSH tunnel without changing image digests. Supervisor pulls and installs the app;
checks bind the running reference, architecture, manifest/config, and registry digest to expected inputs. The lifecycle
driver waits for the expected Store `version_latest`, including after a reload request times out. `installation.json`
records these identities and the repository URL. The archive hash, config digest, and registry manifest/index digest
remain distinct.

Without a repository option, the runner uses Supervisor's local-app directory. For a staged remote candidate, supply its
branch URL, image repository, and independently obtained pull digest:

```sh
just hass::haos-app-check \
    --app-archive /nix/store/IMAGE-garmin-toolkit-hass.oci.tar \
    --repository-url 'https://github.com/kubijo/garmin-toolkit#CANDIDATE_BRANCH' \
    --registry-image ghcr.io/kubijo/garmin-toolkit-hass-CANDIDATE \
    --registry-digest sha256:EXPECTED_PULL_DIGEST \
    --timeout 1800
```

Remote metadata stays unchanged and must select the expected image. The URL must allow anonymous HTTP(S), contain no
credentials, and select a branch; the digest must be SHA-256. `--repository` and `--repository-url` are mutually
exclusive.

The runner uses dark screenshots and retains evidence in `.tmp/haos-evidence/`. Success or failure removes its VM,
browser, credentials, listeners, and disks. It needs no real HASS instance or USB device.

Add `--install-links` with the public `https://github.com/kubijo/garmin-toolkit#hass-apps` repository and the release's
independently verified registry digest to test the README/quickstart links. In that disposable guest only, the check
uninstalls its empty test app and removes the repository, then follows My Home Assistant to add it, install/start the
app, and reopen the existing repository. Home Assistant/My Home Assistant have no Garmin semantic controls, so this part
uses the owned DevTools connection; app actions and captures still use the built-in APIs. Dark screenshots and
`install-links.json` accompany the digest-bound installation and populated lifecycle evidence.

Release qualification is recorded in [publication evidence](../../../docs/research/publication-readiness.md);
[physical-device acceptance](../../../docs/plans/hass-watch-vertical-slice.md) is separate.

## Home Assistant app package

The `garmin-hass-app-image` flake output packages the production host and browser assets as a native OCI archive for the
builder's Linux architecture. [App metadata](../../../apps/garmin-hass/addon/config.yaml) declares AMD64 and ARM64, raw
USB access, ingress on port 8099, and a cold backup of persistent `/data`. The image selects raw MTP directly and
accepts HTTP and WebSocket connections only from the Supervisor ingress gateway. The ordinary standalone production host
retains its desktop-mounted MTP source.

For standalone hosting, `GARMIN_TOOLKIT_HASS_ADDRESS` overrides `127.0.0.1:8099`, and `GARMIN_TOOLKIT_HASS_WEB_ROOT`
overrides the package's bundled frontend default. A missing configured frontend fails startup.
`GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY` restricts requests to that socket peer when set; invalid addresses fail startup.
The host's startup tests run the real HTTP server with a fake device source in isolated subprocesses, covering custom
data/frontend roots, listener overrides, and allowed/rejected ingress peers in production and demo builds. Full QA and
CI include both configurations.

QEMU's directory-backed MTP emulator uses vendor ID `0x46f4`, while production discovery requires Garmin's `0x091e`.
Full attachment testing needs a VID-capable emulator or a passed-through device.

## Interactive HAOS app session

Use the app archive matching this host's CPU architecture:

```bash
just hass::haos-app-run --app-archive /nix/store/...-garmin-toolkit-hass.oci.tar
just hass::haos-app-stop SESSION_ID
```

The runner validates the app archive and selects matching HAOS, QEMU, and firmware. `--arch auto` defaults to the host;
`--arch amd64` and `--arch aarch64` override it. The smoke command accepts the same selector. Native guests use KVM when
accessible, otherwise TCG.

After a headless Chrome capture preflight, the runner boots HAOS, onboards `admin` / `admin`, installs the app, and
verifies authenticated ingress. It prints the URL and credentials for manual inspection. `--no-verify-browser` skips
browser verification. Startup allows thirty minutes; inspection allows two hours, adjustable with `--session-timeout`.
Stopping, interrupting, or exceeding either deadline cleans up. `--port` selects the loopback HTTP listener, forwarded
through an owned SSH tunnel to Core's reported port.

The VM has four CPUs, four GiB guest RAM, a verified six-GiB cgroup limit, and a sixteen-GiB overlay limit. Runtime
files stay under repository `.tmp`; evidence includes versions, screenshots, and browser diagnostics. The browser helper
uses its owned DevTools socket to verify the canvas/Remoc connection and capture initial ingress. Production HTTP
control routes stay disabled; the populated workflow's automation is described below.

## Populated HAOS workflow

For a populated workflow through authenticated ingress, run:

```bash
just hass::haos-app-check \
    --app-archive .tmp/hass-native-build/garmin-toolkit-hass-VERSION-amd64.oci.tar \
    --build-receipt .tmp/hass-native-build/native-build.json \
    --repository /nix/store/REPOSITORY-garmin-hass-app-repository
```

Keep the receipt, archive, and SBOM together. `--build-receipt` verifies their filesystem contract, payload hashes,
architecture/config digest, and SBOM against committed source version, lockfile, and notices. Fetch the recorded commit
first: PR builds record GitHub's tested merge commit. The retained receipt is unsigned; local images can omit it.

`--timeout` bounds startup and the populated workflow separately, defaulting to thirty minutes each. The workflow
deadline starts after authenticated ingress is verified and includes all restarts, reboots, and recovery checks;
individual operations do not reset it. Onboarding requests use the remaining startup budget without retrying mutations.
The VM and host controller have independent lifetime limits and retain evidence outside disposable scratch.

The test creates a profile, imports the [GPX fixture](../../../crates/garmin-gpx/tests/fixtures/candidates.gpx),
generates a FIT Course, and downloads both artifacts. It repeats state/download checks after app restart, HAOS reboot,
and cold backup/restore. Reboot must change the kernel boot ID and automatically start the same image before ingress
reopens. An independent oracle reads the stopped app's SQLite database and WAL: downloads must match stored artifacts,
and GPX must match the fixture. Restore must also reverse the post-backup Course deletion and added profile.

The unmodified production frontend is checked first. The isolated browser then enables the bundled semantic driver
through its canvas attribute. Actions and screenshots use built-in APIs; DevTools handles file choosers and downloads.
When ingress has no app canvas, DevTools captures the Home Assistant error page. Evidence includes synthetic downloads,
`workflow.json`, and each capture's semantic targets (`*-targets.json`). Browser-compositor images immediately before
(`*-presented.png`) and after (`*-presented-after.png`) app-owned readback use the same ingress viewport, allowing
visual comparison with the app-owned PNG. Cleanup follows the runner contract above.

Add `--lifecycle` with a generated repository and native receipt to test first-release upgrade/recovery. A private
`0.0.0` fixture keeps the candidate's filesystem layers and changes only image metadata. Supervisor must discover and
install the unchanged candidate as an update, preserving state and exact downloads. The test then uninstalls the app,
checks its container/private data are gone and its backup remains, and verifies an empty reinstall. Restoring the
pre-upgrade backup on the fixture must recover the original data; a second upgrade must retain it. Image identities are
recorded in `lifecycle-images.json`. This qualifies Supervisor lifecycle behavior, not older-binary/schema migration.

To qualify an upgrade from a published binary, also pass `--baseline-release DIRECTORY`. Download that release's
`release.json`, `SHA256SUMS`, `checksums.sigstore.jsonl`, `artifacts.sigstore.jsonl`, and matching OCI image into the
directory; fetch its version tag into the local Git history. The check authenticates checksums, release manifest, and
image against the canonical release workflow and exact tagged source commit before booting HAOS. It installs the
unchanged published image, creates the populated data and backup there, then exercises both upgrades and recovery with
the unchanged candidate. Evidence identifies the published baseline and reports binary migration coverage separately
from fixture coverage. It covers the synthetic profile, route, and Course data exercised by this workflow. Courses from
an older encoder remain under "Previous FIT files"; the browser opens that section and downloads the original bytes
without regenerating the Course.

When both archives declare the same version, pass a greater private `--candidate-version`. Only the disposable
repository metadata and guest registry tag use that version; OCI configuration, manifests, layers, and original archive
versions remain unchanged. Release qualification requires the eventual release image and version.

On Supervisor 2026.10.2, recovery restarts Supervisor after restoring a reinstalled app: restore replaces the ingress
token but leaves the routing map stale, producing HTTP 503. The
[restore implementation](https://github.com/home-assistant/supervisor/blob/2026.10.2/supervisor/apps/app.py#L1679) and
[ingress map](https://github.com/home-assistant/supervisor/blob/2026.10.2/supervisor/ingress.py#L125) identify the
upstream limitation. The app image and stored data need no changes.

### Withdrawal and replacement

Use `--withdrawal` instead of `--lifecycle` to test the
[withdrawal protocol](../../release/README.md#release-withdrawal) in the guest's private Git repository. Supply
generated repository metadata, a native receipt, and a greater numeric candidate version:

```bash
just hass::haos-app-check \
    --app-archive .tmp/hass-native-build/garmin-toolkit-hass-VERSION-amd64.oci.tar \
    --build-receipt .tmp/hass-native-build/native-build.json \
    --repository /nix/store/REPOSITORY-garmin-hass-app-repository \
    --baseline-release .tmp/published-baseline \
    --candidate-version NEWER_VERSION \
    --withdrawal
```

The runner populates the baseline through ingress, qualifies its private publication, then withdraws it using production
code. Supervisor refresh must hide the app while its installed image, running state, profile/route/Course data, and
exact downloads survive. Matching retries must preserve both Git refs; stale promotions must fail before and after
replacement qualification.

Production leases advertise the greater candidate. Upgrade, restart, HAOS reboot, and cold restore must preserve data
and downloads; restore must reverse post-backup mutations. Only then is the replacement qualified. `workflow.json`
records coverage alongside store/app responses, journals, image identities, and app-owned captures. The
[HTTP consumer test](../../python/test_hass_repository.py) guards private-store isolation and branch continuity.

Only the private registry tag and Supervisor version change; candidate OCI bytes stay unchanged. Without
`--baseline-release`, the baseline is the metadata-only `0.0.0` fixture above. Public GitHub/GHCR refs remain untouched;
their failure recovery is separate.

### Signed installation recovery

Use `--recovery` with two already published, signed releases to test an actual Supervisor update failure and retry:

```bash
just hass::haos-app-check \
    --app-archive .tmp/recovery-candidate/garmin-toolkit-hass-VERSION-amd64.oci.tar \
    --repository /nix/store/REPOSITORY-garmin-hass-app-repository \
    --baseline-release .tmp/recovery-baseline \
    --candidate-release .tmp/recovery-candidate \
    --recovery
```

Populate both directories with the signed inputs described above. Images are authenticated before boot; the candidate
must be newer than the baseline, match the checkout's app version, and use the selected archive. Private version aliases
and other lifecycle modes are rejected. Signed published inputs replace the unsigned native receipt for this mode.

The runner populates and qualifies the private baseline, advertises the candidate with production leases, then stops
only the guest's fixture registry and verifies its Docker state. Supervisor must attempt an uncached update and return
`app_unknown_error` with the target app in `extra_fields`. The CLI must deliver that raw JSON successfully; transport
failures, other error keys or apps, malformed responses, timeouts, and unexpected update success fail the check.
Registry restoration and metadata rollback run even when the request fails unexpectedly.

This uses Supervisor's
[structured error contract](https://contributors.home-assistant.io/apps/supervisor-api/#responses). Supervisor wraps
image-pull errors as `app_unknown_error`; it exposes no distinct connection-refused key. The evidence therefore records
an update failure during the injected registry outage and retains the human message as diagnostics.

Supervisor must rediscover the exact baseline metadata while its installed image, data, and downloads survive. The
unchanged candidate is retried, then checked through app restart, HAOS reboot, and cold restore. Only after those checks
and retained-input hashes pass is the retry qualified; stale rollback and matching retry must preserve both Git refs.
`workflow.json` records the signed identities and failure response alongside the publication journals and captures.

This covers signed image installation and production store recovery using a private Git repository. It authenticates the
selected images, manifests, checksums, and signature bundles; dependency source bundles are outside this check. Public
GitHub/GHCR refs remain unchanged.

See [recovery evidence](../../../docs/research/publication-readiness.md#recovery-checks) for tested inputs and scope.

## HAOS app smoke

Build the matching `.#garmin-hass-app-image` with a memory-capped Nix builder, then run:

```bash
just hass::haos-app-smoke --app-archive /nix/store/...-garmin-toolkit-hass.oci.tar --timeout 1200
```

The runner installs the unchanged OCI image in pinned HAOS. It checks gateway HTTP 200, non-gateway HTTP 403, and no
published host port. It also opens an alternate storage generation with the original database absent, then verifies
Supervisor cold backup/restore of that selection.

The VM has four CPUs, a six-GiB memory cap, and an eight-GiB overlay under repository `.tmp`; cleanup removes the VM and
disks. It uses the landing page without Core onboarding. Use the populated workflow for authenticated ingress
acceptance.

## ARM64 candidate acceptance

Run the populated lifecycle check locally using a downloaded `garmin-hass-native-aarch64` CI artifact, containing the
OCI image, SPDX inventory, and `native-build.json`:

```bash
nix run "git+file://$PWD#hass-ci" -- acceptance \
    --system aarch64-linux \
    --build-directory .tmp/arm64-native \
    --evidence-directory .tmp/arm64-acceptance \
    --image .tmp/haos-aarch64.qcow2.xz
```

The receipt's source commit must be available in the local Git history, including the CI merge commit for a PR build.
The command verifies the archive, inventory, source, and selected architecture before starting a VM. It generates an
ARM64-only private Store using `garmin-hass-arm64-test-repository`; release candidates qualify both architectures
separately. Omit `--image` to download the pinned HAOS image. Owned-process cleanup follows the acceptance deadline.

The `0.0.0` metadata fixture qualifies Supervisor lifecycle behavior; it does not exercise older-binary/schema migration
or remote delivery.

### Staged ARM64 delivery

The [release workflow](../../../.github/workflows/hass-release.yml) reuses a retained ARM64 artifact from a successful
CI run:

```bash
gh workflow run hass-release.yml --ref BRANCH -f arm64_run=SOURCE_CI_RUN_ID
```

The run's head must be an ancestor of the dispatched checkout and its native artifact must remain available. Before
registry login, checks bind the source run, head or PR merge, receipt, archive, and SPDX inventory. Fresh emulated ARM64
HAOS installs through a temporary Git branch and anonymous GHCR pulls. Populated ingress, restart, reboot, cold restore,
and exact downloads must pass; evidence binds the installed index/manifest/config to the retained build.

This mode skips native builds, the full CI matrix, signing, and promotion. Cleanup removes owned processes, credentials,
and the unchanged candidate branch; the candidate image tag remains. Reports and screenshots are uploaded as
`hass-arm64-delivery`. Public Store metadata and release tags must remain unchanged. Each combined release index needs
its own acceptance on both architectures; see
[release evidence](../../../docs/research/publication-readiness.md#published-amd64arm64-release).

## ARM64 HAOS boot smoke

Run a fresh, networkless Home Assistant OS ARM64 guest on an x86 Linux host with QEMU software emulation:

```bash
just hass::haos-arm64-smoke
```

The runner verifies the [pinned HAOS image](../../python/haos_platform.py) SHA-256, then checks GRUB Slot A and the
mounted data partition. Reuse a download with `--image /path/to/haos-aarch64.qcow2.xz`.

The networkless guest has no USB access, verified four-CPU/six-GiB limits, and a two-GiB overlay. Runtime files stay
under repository `.tmp` and are removed afterward. This checks OS boot only; app installation, ingress, and hardware
need separate acceptance.

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
enabled or disabled, and tracing enabled or disabled. Cases alternate abrupt/graceful host restarts and verify process
and state cleanup. Constructor counters assert standard WebSocket selection. This diagnostic is separate from CI's route
workflow check.

The application selects `websocket-web`'s standard interface to avoid a Chromium 153.0.8010.12 native reconnect crash
with automatic WebSocketStream selection, reproducible without fault hooks or tracing. Bare sockets did not reproduce
it. Preserve the library's buffering and transport semantics when changing this selection.

Both suites build the packaged demo and run a disposable NixOS VM with two CPUs and 4 GiB RAM. The route suite is also a
Linux flake check, so CI's `nix flake check` runs it; ordinary `qa::full` does not boot a VM. The local recipe runs the
Nix-built driver as your user so it can use your KVM access; CI runs it in the Nix builder. Both require hardware
acceleration and refuse CPU emulation. Build and VM output go to separate logs in the printed diagnostics directory;
cancellation stops the owned process group before removing temporary VM state. New files must be Git-tracked for the
repository's Git-backed flake to include them.

Host-user KVM access does not imply that a sandboxed Nix build user can open `/dev/kvm`. If the flake check fails before
the QEMU monitor connects with a KVM permission error, use `just hass::integration` to run the same suite under your
user. Pass `--driver /nix/store/DRIVER-nixos-test-driver-hass-routes-integration` to reuse its already-built driver
without another build. A complete local `nix flake check` still requires KVM permission for the actual Nix builder; CI
sets this up explicitly. VM disks and sockets stay under repository `.tmp` and are removed after failure as well as
success; the diagnostic logs and copied test artifacts remain.

For a multi-user Linux installation using the `nixbld` build group, persist builder access with
`/etc/udev/rules.d/99-nix-kvm.rules`:

```udev
SUBSYSTEM=="misc", KERNEL=="kvm", RUN+="/usr/bin/setfacl -m g:nixbld:rw /dev/kvm"
```

Confirm the build group and `setfacl` path on the host first. The named group ACL preserves the device's existing
`root:kvm` ownership and desktop-user access without granting access to every local user. It is reapplied when udev
creates the device, including after reboot. Reload the rules and apply this one to the existing device:

```bash
sudo udevadm control --reload-rules
sudo udevadm trigger --action=change --sysname-match=kvm
sudo udevadm settle
getfacl /dev/kvm
```

The ACL must include `group:nixbld:rw-`. Nix's sandbox drops supplementary groups, so adding build accounts to `kvm`
does not reliably grant sandbox access. This rule assumes fixed build users with primary group `nixbld`; automatic
UID/GID allocation requires a policy matching those identities instead. Verify with the actual sandboxed integration
check, not only a command run as the logged-in user. Keep hardware acceleration mandatory so a permission regression
fails instead of falling back to slow CPU emulation.

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
State is disposable; diagnostics are retained.
