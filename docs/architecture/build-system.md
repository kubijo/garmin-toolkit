# Build system

The root flake pins toolchains, dependencies, checks, and filtered workspace source. App flakes own packaging; the root
re-exports `garmin-cli` (default), `desktop`, `desktop-demo`, `garmin-hass`, `garmin-hass-demo`, and `gallery`. Follow
[development safeguards](../development.md). Just commands use `namespace::recipe`.

CI builds the sandboxed source-closure check and runs `nix flake check -L --no-update-lock-file`. The lockfile is an
explicit input, not an update side effect of the quality gate.

## Checks and outputs

| Command                | Scope                                                                        |
| ---------------------- | ---------------------------------------------------------------------------- |
| `just qa::preflight`   | Formatting, repository policy, Grit; no Rust compilation                     |
| `just qa::full`        | Preflight, Rust and gallery lint/tests, documentation and generated metadata |
| `just qa::wasm`        | Actual browser target lint                                                   |
| `just qa::audit`       | RustSec and Gitleaks history/index/worktree scans                            |
| `just qa::check`       | Pure Nix checks and application builds                                       |
| `just qa::trunk-cache` | Isolated real-WASM bindgen-cache acceptance                                  |

Coverage has a 30% global floor; [release work](../plans/README.md) owns per-owner floors. Generated licenses live in
`assets/licenses`; `crates/garmin-brand/assets/icon.svg` is the sole app-icon source. AppImage/Flatpak exports use
`dist/`. Cargo caches use `.tmp/cargo-target` for ambient builds and `.tmp/nix-cargo-target` for Nix-shell builds.
Interactive gallery sessions own `.tmp/gallery-target`; captures, tests, checks, and profiling use
`.tmp/gallery-check-target`. Never direct another build into a live session's target directory: the gallery watches its
scene library and loads replacements without checking compiler compatibility. Restart gallery after changing toolchains
or dependency configuration.

Gallery recipes add pinned FormatJS to the executable path, including hot-reload child builds. They retain the caller's
Rust toolchain and graphics runtime; entering the full Nix development shell remains explicit.

Root and gallery apply the same `vendor/` patches: bounded MVT decoding in `fast-mvt`, Wayland activation in `winit`.
Each has `PATCHES.md`. Nix sources, license checks, and dependency-cache builds retain full patched sources/manifests.
Headless Linux GPU checks use pinned Mesa software Vulkan and fail if no adapter exists. Tests use pinned timezone data
through `TZDIR`, including sandboxed tests and coverage.

CI uses [Magic Nix Cache](https://github.com/DeterminateSystems/magic-nix-cache-action) to cache individual Nix store
paths in GitHub Actions storage. Completed builds can be saved even when later checks fail; unfinished derivations
cannot. FlakeHub caching and diagnostic uploads are disabled. Cache population and speed must be verified in CI.

## Development build timing

```sh
just dev::build-profile desktop LABEL
just dev::build-profile web LABEL
just dev::compiler-profile ui LABEL
just dev::compiler-profile desktop LABEL
just gallery::run --hot
```

Build profiling accepts `web`, `desktop`, `hass`, `gallery`, `cli`, and `ui`; labels must be unique. Reports in
`.tmp/build-profile/LABEL` contain HTML/Perfetto timelines, Cargo timings, commands, versions, and exit status.
`--incremental 0` and `--config` support controlled comparisons. The wrapper preserves Cargo jobserver descriptors and
forwards termination signals to the build process group.

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

HASS caches recognized fingerprinted assets immutably; HTML/errors use `no-store`. Deploy the whole bundle and restart
HASS. `fingerprint-web.test.mjs` checks graph invalidation and emitted inventory; `GARMIN_TEST_WEB_ROOT` selects a built
bundle for emitted tests, including `map-worker.test.mjs`.

## Platforms and browser runs

The root supports both Linux architectures and Apple Silicon macOS. `nix develop` supplies native/WASM Rust, C/C++,
CMake, Trunk, and matching bindgen; `.#hass` supplies the HASS shell. macOS QA also pins Make, the Apple SDK, and the
deployment target. Wrappers enforce serial builds on macOS; Linux additionally enforces a memory cap. AppImage and
Flatpak remain Linux-only. Development support does not establish hardware compatibility.

Gallery uses Metal on macOS and Vulkan on Linux; override with `WGPU_BACKEND`. For browser automation, control, capture,
and diagnostics see [developer tools](developer-tools.md). Renderer selection and ownership live in
[activity map](activity-map.md#rendering-boundary).

## Desktop packaging

AppImage and Flatpak target Linux `x86_64` and `aarch64`. The x86_64 demo AppImage has passed a packaged NVIDIA/Vulkan
smoke run; neither format has completed release validation. Production and demo have separate application IDs and
platform data:

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
host graphics drivers or desktop services: release validation must cover WGPU rendering, file dialogs, and USB/GVfs
access on non-Nix systems. See [publication work](../plans/device-expansion-and-publication.md).

Host graphics integration is explicitly outside
[nix-appimage's scope](https://github.com/ralismark/nix-appimage#caveats). On 2026-10-05, the x86_64 demo AppImage using
the automatic wrapper reported Vulkan on an NVIDIA GeForce RTX 4090. Process mappings confirmed bundled libc and Vulkan
loader with the host NVIDIA 595.91.07 driver. The packaged `activity-smoke` scenario passed all 55 actions in 20.65
seconds, covering map interaction, playback, laps, scrubbing, and activity replacement/restoration. On the same Ubuntu
26.04 host, Mesa 26.0.3 llvmpipe Vulkan passed all 60 actions with two worker threads and functional/background timing;
the normal timing run failed an interaction deadline. This is software-renderer functionality evidence, not performance
evidence. Forcing the AMD GPU failed: Wayland rejected imported DMA buffers and XWayland reported an invalid surface.
AMD hardware presentation remains unresolved. The connected display is on NVIDIA; all AMD display outputs are
disconnected. The forced-AMD test therefore exercises cross-GPU presentation. Mutter emits the observed Wayland error
when importing the client buffer fails; egui's surface-configuration panic follows that rejection. Adapter selection
already requests a compatible surface. Running the same host-graphics-wrapped executable outside the AppImage reproduced
the Wayland buffer-import rejection and surface-loss panic (exit 101). AppImage mounting and namespace isolation are
therefore not required to reproduce it; the same DMA-buffer import error also occurred with Homebrew's
`vkcube --wsi wayland`, forced to the host Radeon ICD. That reproduction selected the AMD integrated GPU without our
application or its Nix/AppImage libraries, establishing a host cross-GPU presentation failure independently of this
package. It does not establish a failure on systems where AMD drives the display or prove the package's library
compatibility on other hosts. On the same host, the locally built
[headless probe](../development.md#headless-hardware-rendering-probe) selected the AMD integrated GPU through RADV/Mesa
26.0.3 with 4x MSAA. Activity and responsive-layout scenarios passed, and captured wide/narrow frames showed maps,
routes, charts, text, and controls. This verifies those rendering paths without presentation; it does not validate the
AppImage library mix or the AMD-to-NVIDIA buffer transfer. The rebuilt demo also passed manual backup saving and
file-dialog cancellation on this host. Opening the log folder launched the host file manager. Developer tools now uses
an independent deferred viewport to remain interactive when the main window is covered; the user reported the fix
working on this host. Regression tests cover child-only cursor updates, section toggling, scrolling, close/reopen, and
control-server lifecycle with the root marked occluded. Library discovery cannot guarantee ABI compatibility with every
host driver; other hosts, aarch64, and production USB/GVfs remain unverified.

Flatpak builds offline from Nix-vendored Cargo sources on Freedesktop 25.08. Its current permissions are development
inputs; release requires verifying the narrowest working USB and GVfs permissions.
