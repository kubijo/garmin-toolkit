# Build system

The root flake pins toolchains, dependencies, checks, and filtered workspace source. App flakes own packaging; the root
re-exports `garmin-cli` (default), `desktop`, `desktop-demo`, `garmin-hass`, `garmin-hass-demo`, and `gallery`. Follow
[development safeguards](../development.md). Just commands use `namespace::recipe`.

## Checks and outputs

| Command                | Scope                                                                        |
| ---------------------- | ---------------------------------------------------------------------------- |
| `just qa::preflight`   | Formatting, repository policy, Grit; no Rust compilation                     |
| `just qa::full`        | Preflight, Rust and gallery lint/tests, documentation and generated metadata |
| `just qa::wasm`        | Actual browser target lint                                                   |
| `just qa::audit`       | RustSec and Gitleaks history/index/worktree scans                            |
| `just qa::check`       | Pure Nix checks and application builds                                       |
| `just qa::trunk-cache` | Isolated real-WASM bindgen-cache acceptance                                  |

Coverage has a 30% global floor; [integration work](../plans/toolkit-integration.md) owns per-owner floors. Generated
licenses live in `assets/licenses`; `crates/garmin-brand/assets/icon.svg` is the sole app-icon source. AppImage/Flatpak
exports use `dist/`. Cargo caches use `.tmp/cargo-target` for ambient builds and `.tmp/nix-cargo-target` for Nix-shell
builds. Interactive gallery sessions own `.tmp/gallery-target`; captures, tests, checks, and profiling use
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
