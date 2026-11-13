# Build system

The root flake pins toolchains, dependencies, checks, and filtered workspace source. App flakes own packaging; the root
re-exports `garmin-cli` (default), `desktop`, `desktop-demo`, `garmin-hass`, `garmin-hass-demo`, and `gallery`. Follow
[development safeguards](../development.md). Just commands use `namespace::recipe`.

The lockfile is an explicit check input, not an update side effect. CI check groups and native image jobs are described
below.

## Checks and outputs

`dev::verify`, `dev::test`, and CI run the workspace with all features plus separate desktop and HASS production suites
with `--no-default-features`. Full QA and CI lint both configurations. All-features builds select demo composition, so
the production checks exercise startup and device adapters that the demo excludes. Host tests cover application
identity, isolated data roots, persisted profiles after restart, and HASS environment overrides using disposable storage
and fake devices.

Coverage enforces per-owner floors from `infra/coverage/owners.toml` in the Nix coverage app and check.
`just dev::coverage::owners .tmp/coverage/lcov.info --policy infra/coverage/owners.toml` checks an existing report.
Nested packages are separate owners; unmeasured owners and unassigned files remain visible. `--output FILE.json` writes
the per-owner and per-file counts for policy review. Line coverage does not establish behavioral correctness. The native
collector excludes `crates/garmin-ui/` and `apps/garmin-desktop/src/view.rs`; browser-only WASM code is unmeasured by
the native run. Owner percentages describe only the files measured by that collector.

Floors are an upward ratchet: raise them when meaningful tests improve coverage. Reports never rewrite policy. Lowering
a floor or expanding an exclusion requires an explicit reason in the reviewed change. This is a review rule; the gate
checks the current policy, not its Git history. New owners require a floor or a documented unmeasured reason. Missing
measurements, stale owners, and newly measured exempt owners fail the gate. Comparisons use exact line counts.

Generated licenses live in `assets/licenses`; `crates/garmin-brand/assets/icon.svg` is the sole app-icon source.
AppImage/Flatpak exports use `dist/`. Ambient builds, Nix development shells, gallery checks and captures, and stable
build profiling share `.tmp/cargo-target`. Interactive gallery sessions use `.tmp/gallery-target`; compiler profiling
keeps its separate nightly target. Never direct another build into a live gallery session's target directory: the
gallery watches its scene library and loads replacements without checking compiler compatibility. Restart gallery after
changing toolchains or dependency configuration. Different toolchains, target triples, profiles, and Cargo features may
still produce distinct artifacts inside the shared directory. Nix package builds keep their isolated sandbox outputs.

Local Just entrypoints, Nix QA apps, and development shells preserve an explicit `RUSTC_WRAPPER`. When it is unset and
the host provides `sccache`, they select that executable; hosts without it continue without a wrapper.

Coverage builds and tests `cargo-llvm-cov` with the selected Rust toolchain and uses that toolchain's `llvm-cov` and
`llvm-profdata` for local and CI reports. Package tests remain enabled.

SQLFluff uses the standard pinned nixpkgs package so tooling can reuse upstream binary caches. Package tests remain
enabled; local dependency overrides would require separate builds and can make initial formatting setup expensive.

The root and gallery workspaces share development-profile overrides in `.cargo/config.toml`. Ordinary code stays
unoptimized; rendering dependencies retain their targeted optimizations. A blanket optimization override in either
workspace would prevent otherwise compatible dependencies from sharing artifacts.

Workspace Nix quality checks use the development profile for both dependency caches and check commands. The caches
compile test dependencies too, so each check can reuse them. This avoids release-profile LTO for test executables;
production package builds retain the release profile.

The CLI package and its dependency cache share the same locked package selection. Building the entire workspace in that
cache would compile unused packages and unify dependency features differently from the final CLI build.

`just dev::cache::report` shows the target caches and read-only totals for the Cargo home and Nix user cache. Root Just
and Nix QA entrypoints make generated build-script outputs in repository target caches writable before reuse. Some
dependency build scripts preserve read-only Nix source permissions when copying bindings into `out`; without this
preparation, their next rebuild cannot overwrite those files. Preparation preserves other artifacts and source files.
`just dev::cache::clean shared` previews removal of the shared target; use `just dev::cache::clean shared apply` to
remove it after builds have stopped. The other accepted names are `gallery-live`, `nightly`, and `legacy`; `legacy`
covers the old Nix-shell, gallery-check, web-profile, and temporary HASS target directories. This command only removes
the named repository target cache. It does not prune Nix store outputs, Cargo's global cache, or build-profile reports.

Gallery recipes add pinned FormatJS to the executable path, including hot-reload child builds. They retain the caller's
Rust toolchain and graphics runtime; entering the full Nix development shell remains explicit.

The [standalone gallery](../../infra/gallery/Cargo.toml) pins `gallery`, `gallery-build`, and `gallery-macros` together,
keeps egui compatible with upstream, and retains native AccessKit accessibility. Toolchain selection lives in
[`rust-toolchain.toml`](../../rust-toolchain.toml); the workspace minimum lives in [`Cargo.toml`](../../Cargo.toml).
Upstream's development pin does not declare a minimum Rust version. The
[gallery audit policy](../../infra/gallery/deny.toml) has three AccessKit/AT-SPI exceptions pending zbus 6 migration;
obsolete exceptions and other findings fail. The application workspace has no such exceptions.

Root and gallery apply the same `vendor/` patches: bounded MVT decoding in `fast-mvt`, Wayland activation and a bounded
control-request redraw override in `winit`. Each has `PATCHES.md`. Nix sources, license checks, and dependency-cache
builds retain full patched sources/manifests. Headless Linux GPU checks use pinned Mesa software Vulkan and fail if no
adapter exists. Tests use pinned timezone data through `TZDIR`, including sandboxed tests and coverage.

CI partitions every flake check into tooling, Rust, desktop, gallery, or HASS groups exposed under
`legacyPackages.SYSTEM.ci`. New checks enter a group automatically. Each group runs in its own job with two CPUs, 6 GiB
of memory, and a 120-minute deadline; package checks retain their release builds. The tooling job also evaluates the
full flake. The final `check` job requires every group and both native HASS image jobs to succeed.

CI uses [Magic Nix Cache](https://github.com/DeterminateSystems/magic-nix-cache-action) to cache individual Nix store
paths in GitHub Actions storage. Completed builds can be saved even when later checks fail; unfinished derivations
cannot. FlakeHub caching and diagnostic uploads are disabled.

## Development build timing

```sh
just dev::profile::build desktop LABEL
just dev::profile::compiler ui LABEL
```

Use unique labels. Reports in `.tmp/build-profile/LABEL` contain HTML/Perfetto timelines, Cargo timings, commands,
versions, and exit status. `--incremental 0` and `--config` support controlled comparisons. The wrapper preserves Cargo
jobserver descriptors and forwards termination signals to the build process group.

Compiler profiling uses the pinned nightly, [measureme](https://github.com/rust-lang/measureme), and the separate
`.tmp/build-profile/nightly-target` cache. Only the selected library receives profiling flags. Reports include raw data,
text/JSON summaries, and `compiler/chrome_profiler.json` (events ≥50 μs). Stable flags and serialized collection prevent
cache invalidation and stale-profile reuse; unchanged builds produce no compiler profile. Default mode is demo;
`--mode production` changes the feature set.

Warm caches before measuring edits; measure unchanged builds separately. Spans overlap, compiler time includes linking,
and reports exclude shell startup and reload. Validate diagnostic-nightly findings with stable builds.
[Recorded measurements](../research/build-performance.md).

Development enables incremental compilation, leaves the UI unoptimized, and retains optimized rendering dependencies.
Linux Just entrypoints and root dev shells use Mold; macOS/WASM and production packaging use their own linkers. Use
direct Cargo inside the pinned shell. Gallery supports scene reload; desktop requires restart.

## Browser asset graph

Development shells use patched Trunk 0.21.14 (`infra/nix/trunk`); packaging uses upstream Trunk. Only development
bindgen 0.2.126 builds cache output, under the Cargo target's `trunk/bindgen-cache/v1`. Keys include WASM, executable
bytes/path/version, arguments, working directory, `WASM_BINDGEN_*`, and each Cargo package's `package.json`. This
bindgen embeds imported JS in WASM and tracks it as a compiler input. Other versions and release builds bypass the
cache; re-audit inputs before widening support.

Output trees publish atomically and are integrity-checked on reuse. Hits still run asset copying, bundling, and
fingerprinting. `TRUNK_BINDGEN_CACHE_DISABLE=1` bypasses caching; deleting the target directory clears it. The
acceptance fixture tests restoration, imported-JS invalidation, and independent asset edits; temporary files are cleaned
on completion and catchable termination signals.

Trunk builds WASM/HTML; its post-build esbuild hook fingerprints the entire dependency graph, including snippets and
static files, and minifies releases. HTML publishes `garmin-module`, `garmin-wasm`, `garmin-map-worker`, and
`garmin-map-render-worker` paths. Hosts/workers consume those paths, never guessed filenames. Bindgen's implicit WASM
URL becomes a file-loader import; an unrecognized generated shape fails the build. No unhashed aliases are published.

HASS serves Brotli/gzip asset variants with `Vary: Accept-Encoding` and caches recognized fingerprinted assets
immutably; HTML/errors use `no-store`. Deploy the whole bundle and restart HASS. `fingerprint-web.test.mjs` checks graph
invalidation and emitted inventory; `GARMIN_TEST_WEB_ROOT` selects a built bundle for emitted tests, including
`map-worker.test.mjs`.

## Platforms and browser runs

The root supports both Linux architectures and Apple Silicon macOS. `nix develop` supplies native/WASM Rust, C/C++,
CMake, Trunk, and matching bindgen; `.#hass` supplies the HASS shell. macOS QA also pins Make, the Apple SDK, and the
deployment target. Wrappers enforce serial builds on macOS; Linux additionally enforces a memory cap. AppImage and
Flatpak remain Linux-only. Development support does not establish hardware compatibility.

Gallery uses Metal on macOS and Vulkan on Linux; override with `WGPU_BACKEND`. For browser automation, control, capture,
and diagnostics see [developer tools](developer-tools.md). Renderer selection and ownership live in
[activity map](activity-map.md#rendering-boundary).

Linux development shells expose Mesa's software Vulkan driver alongside the host drivers through `VK_ADD_DRIVER_FILES`.
Headless QA selects the pinned software driver explicitly through `VK_DRIVER_FILES`. Linux shells and QA use the pinned
Nix C compiler and native Rust linker so their binaries can load that driver's glibc generation; the target-specific
Mold settings remain in effect.

## Desktop packaging

AppImage and Flatpak target Linux `x86_64` and `aarch64`. Production and demo have separate application IDs and platform
data:

| Mode       | Application ID                 | AppImage                                 |
| ---------- | ------------------------------ | ---------------------------------------- |
| Production | `io.kubijo.GarminToolkit`      | `dist/garmin-toolkit-ARCH.AppImage`      |
| Demo       | `io.kubijo.GarminToolkit.Demo` | `dist/garmin-toolkit-demo-ARCH.AppImage` |

Flatpak outputs use the application ID with a `.flatpak` suffix. Build with `just desktop::appimage production|demo` or
`just desktop::flatpak production|demo`. Both formats consume generated launcher metadata and brand assets. AppImage
uses the pinned [nix-appimage builder](https://github.com/ralismark/nix-appimage) with the normal desktop package. The
builder preserves the complete Nix runtime closure, including libc, libraries, helper programs, and resources. Both
modes use the same builder and the project's nixpkgs pin. Runtime library and GIO module paths belong to the desktop
package wrapper so they remain available outside the development shell and are included in the closure.

Both AppImage modes use the reusable [host graphics launcher](../../infra/nix/host-graphics/default.nix). It places
bundled libc and the application's runtime closure library directories before inherited and host directories in
`LD_LIBRARY_PATH`. The bundled `ldconfig` reads the host's `/etc/ld.so.cache` without modifying it, discovering vendor
and multiarch library directories at launch. Standard directories and NixOS's `/run/opengl-driver/lib` provide
fallbacks. The wrapper preserves launcher metadata and forwards arguments, signals, and the application's exit status.
It snapshots the incoming library, GIO-module, and XDG-data search paths before the application wrapper modifies them.
Host folder launches restore those values, including the distinction between unset and empty, so host executables do not
inherit bundled libc or GIO modules. New native process launches must use the same environment boundary.

The AppImage launcher requires unprivileged user namespaces. Bundling the closure does not establish compatibility with
host graphics drivers or desktop services; supported rendering, file-dialog, and USB/GVfs tuples are recorded in
[packaging evidence](../research/desktop-packaging.md).

Host graphics integration is outside [nix-appimage's scope](https://github.com/ralismark/nix-appimage#caveats).

`just desktop::vm-test --usb VENDOR:PRODUCT` builds the production ARM64 AppImage and boots a disposable Ubuntu 26.04
desktop for the complete device acceptance case. On Apple Silicon, Nix's configured Linux builder handles the build;
QEMU uses HVF for the separate non-Nix test guest. The image checksum, dated package snapshot, cloud-init configuration,
firmware, and QEMU machine are pinned in `infra/integration/desktop` and `infra/nix/desktop-vm.nix`. The guest grants
the test AppImage its required user namespaces through a per-application AppArmor profile; the global Ubuntu namespace
restriction stays enabled.

The runner requires exactly one host USB device matching the hexadecimal selector. It opens macOS Screen Sharing for the
guest and asks for discovery, Activities browsing, FIT preview, and dismissal checks. Startup alone cannot pass.
`--appimage PATH` reuses an existing ARM64 artifact. Logs, package inventory, screenshots, and `result.json` remain
under `.tmp/desktop-vm`; the guest disk and SSH credentials are removed when the session ends. The test guest needs
network access to install its pinned packages. Physical GPU acceleration is outside this software-rendered VM case.

The experimental Flatpak tooling builds offline from Nix-vendored Cargo sources on Freedesktop 25.08. Flatpak is an idea
outside the active roadmap; its current permissions are development inputs.

Production and demo AppImages for x86_64 and aarch64 are distributed as GitHub Release assets with matching `.zsync`
files and `DESKTOP-SHA256SUMS`. Each AppImage embeds an architecture- and mode-specific `gh-releases-zsync` channel for
the latest published release of `kubijo/garmin-toolkit`, including prereleases, through Gear Lever's `latest-all`
policy. Gear Lever discovers updates without manual setup. Local AppImage exports include the matching `.zsync` file.
See [desktop release publication](../../infra/release/README.md#desktop-appimages).
