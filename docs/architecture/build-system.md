# Build system

The root flake pins Rust 1.98, dependencies, checks, and deliverables. App flakes own target packaging; the root
supplies filtered workspace source and re-exports their outputs.

Follow the [development safeguards](../development.md) before launching builds, applications, or physical-device checks.

Main packages:

- `garmin-cli`, the default package;
- `desktop` and `desktop-demo`;
- `garmin-hass` and `garmin-hass-demo`;
- `gallery`, a development-only scene host.

`just cli::run` starts the production CLI; `just cli::demo` substitutes disposable adapters. Desktop and HASS recipes
require `production` or `demo`. AppImage and Flatpak exporters write ignored files under `dist/`.

`just qa::preflight` checks formatting and lightweight repository policy, including every Grit pattern test and gated
scan, without compiling the Rust workspace. `just qa::lint` runs repository policy first and only then realizes and runs
the complete Rust and gallery lint suite. `just qa::full` adds the formatting check before that same heavyweight suite.
`just qa::audit` adds RustSec and Gitleaks scans of history, index, and worktree. User-facing module commands always use
`namespace::recipe`; the source-shape gate rejects the ambiguous space form. `nix flake check` builds the pure checks
and application targets.

Coverage currently has a 30% global floor because hardware-only MTP paths depress the merged result. The active plan
replaces it with per-owner floors while retaining imported 80% floors.

`crates/garmin-brand/assets/icon.svg` is the sole app-icon source. Generated license bundles live under
`assets/licenses`. Ambient Cargo uses `.tmp/cargo-target`; Nix shells use `.tmp/nix-cargo-target`.

Cargo patches in `vendor/` apply to both the root workspace and the standalone gallery. `fast-mvt` contains bounded
decoding fixes; `winit` supplies Wayland activation for existing child windows. Each directory records its changes in
`PATCHES.md`. Nix build and license sources include these directories in full, including upstream notices and non-Rust
resources.

Linux Nix test applications, sandboxed Rust tests, and coverage select the pinned Mesa software Vulkan ICD for headless
renderer tests. Those tests must not silently skip when an adapter is unavailable.

## Browser asset graph

Trunk compiles WASM and generates the entrypoint. Its post-build hook runs the pinned esbuild with dependency-aware
`[hash]` entry, chunk, and file-loader names. Trunk's `copy-file` outputs and wasm-bindgen snippet directory names alone
are not sufficient cache identities. The final gate includes all copied modules, snippets, WASM, and static resources;
esbuild rewrites JavaScript imports and supplies emitted paths through its metafile. Release builds minify at this final
bundling step.

The gate rewrites the generated HTML and publishes `garmin-module`, `garmin-wasm`, `garmin-map-worker`, and
`garmin-map-render-worker` meta entries. Browser startup and both worker tiers consume these paths; they must not guess
filenames. The bindgen implicit WASM URL is adapted to a file-loader import, with a build failure if its generated shape
changes. No unhashed asset aliases are published. HASS serves HTML and unsuccessful responses with `no-store`; only
recognized fingerprinted static assets receive immutable caching. Deploy the complete bundle and restart HASS, which
loads its entrypoint at startup.

`infra/javascript/fingerprint-web.test.mjs` exercises cached build-A/build-B dependency changes, binary changes,
repeat-build stability, implicit WASM resolution, and the emitted bundle inventory. Run emitted checks with
`GARMIN_TEST_WEB_ROOT` pointing at the final distribution.

Connect IQ remains a future, local-only integration under ADR 0009.

## Apple Silicon macOS development

The flake exposes `aarch64-darwin` packages, development shells, and QA tools alongside both Linux architectures. CLI,
desktop, HASS, and gallery use the same pinned dependencies and Just entrypoints. AppImage and Flatpak packaging remain
Linux-only. This development support does not establish physical-device compatibility on macOS.

Enter the default environment with `nix develop`; it includes Bash, the native C/C++ compiler, CMake, native Rust, the
WASM target, Trunk, and the matching wasm-bindgen CLI. The HASS shell is `nix develop .#hass`. Use the existing
`just qa::preflight`, `just qa::full`, and `just qa::wasm` gates. Gallery captures choose Metal on macOS and Vulkan on
Linux; `WGPU_BACKEND` overrides that default. GPU checks require a working adapter and must not silently skip failures.

macOS QA applications pin the C/C++ compiler, CMake, Make, Apple SDK, and deployment target as well as Rust; they do not
depend on Homebrew or the selected system Xcode installation. To retry only the gallery Clippy stage, run
`infra/just/memory-capped.sh nix run .#gallery-lint` from the repository root.

On macOS, maintained build wrappers force serial Cargo and Nix builds, including one core per Nix builder. There is no
hard memory cap; this is the approved macOS exception in the development safeguards. For direct Nix builds use
`infra/just/memory-capped.sh nix build` with the filtered Git flake, rather than bypassing those limits.

To resume browser acceptance from the repository root, use a disposable data directory:

```sh
just hass::run demo "$PWD/.tmp/hass-ui-automation" --ui-automation
```

Open `http://127.0.0.1:8099/?map-render-mode=worker-gl` in Chrome on macOS; compare with `?map-render-mode=main-gl`
using the same server. The interaction-run contract below applies to both platforms.

HASS uses the worker-owned OffscreenCanvas WebGL2 map by default in both production and demo builds. No renderer flag or
URL parameter is required. `?map-render-mode=main-gl` selects the matched main-thread WebGL2 baseline;
`--no-map-render-worker` restores the original renderer and disables URL renderer selection for that host. Unsupported
worker initialization reports a renderer error rather than silently substituting a backend.

## Opt-in browser interaction runs

Demo HASS builds accept `--ui-automation`; production hosts reject it before opening storage. It is independent of
renderer selection. Without it, the driver plugin and `window.garminAutomation` API are not installed.

Open Developer tools using the icon beside Profiles and select a scenario in Automation, or call the browser API. Both
launch paths use the same bridge, metadata and trace markers. Developer tools is always available, including on the
profile chooser; automation requires the opt-in driver. Use a wide desktop viewport. Focus changes do not stop a run;
hidden tabs pause it. From an active profile, the runner first operates the profile menu and Log out using ordinary
input, then selects the demo profile again. This reset phase is included in the action report. For matched captures,
start every run from the same screen (prefer a reload to the chooser). The built-in scenarios select the first demo
profile/activity using locale-independent AccessKit author IDs. The same Rust scenario engine runs in headless tests and
the live egui application. It queries the current accessibility tree, uses clipped logical bounds, and injects ordinary
pointer input. It does not call camera setters or replace the application event loop.

The browser API provides `list()`, `start(name)`, `status()`, `result()`, `cancel()`, `targets()`, `action(request)`,
and `sequence(actions)`. The [developer tools contract](developer-tools.md) describes individual actions, native HTTP
control, and application logs. Available names:

- `stationary-arrival`: visible content preparation/upload readiness, then eight seconds of stationary observation;
- `warm-interaction`: readiness, four fixed pan/zoom/fit cycles, then returned-view readiness;
- `activity-smoke`: the warm workload followed by playback/speed assertions, lap selection/reset, chart scrubbing,
  replacement with the indoor/no-GPS ride, and restoration of the original map;
- `responsive-layout`: selection, drawer access, and playback checks across narrow/wide layouts; functional evidence
  only.

Start one Chrome trace with automatic stopping disabled, start one scenario through the API, poll its terminal result,
then stop and save the trace and JSON result together. Reload before the next run. MCP should not pace individual
gestures. Reports include action counts and timing, readiness, renderer identity, viewport/DPR and measured driver/tree
CPU overhead. `garmin.automation.start` and `garmin.automation.phase` are browser performance marks. Phase observation
is polled every 100 ms; authoritative action times are the Rust report, not the polling timestamp. The overhead counters
cover the input and tree-consumption hooks, not total AccessKit generation or status-view cost; establish total
instrumentation overhead separately before drawing performance conclusions. `just hass::profile-analyze TRACE --json`
retains that terminal report and rejects inconsistent passing action counts. Scripted egui input does not produce DOM
dispatch events, so the tool's legacy DOM interaction-frame statistics do not describe these runs; its diagnostic output
explicitly calls out that limitation.

Action reports retain `target_bounds` (`left, top, right, bottom`) and the latest injected `pointer_position`, in egui
logical points. The opt-in status view highlights the last pointer action with a target outline, semantic ID, and
crosshair on egui's debug paint layer. It creates no input region and follows injected drags during the run. AccessKit's
root pixel-scale transform is removed before clipping or injecting input. Emitting a click does not prove its target
accepted it; subsequent semantic assertions establish the expected transition. Overlay painting is instrumentation and
is not included in the input/tree-hook CPU counters.

During a run the Rust egui input hook blocks real pointer, keyboard, wheel and touch input; Stop or Escape cancels.
Focus loss does not cancel. Hidden browser tabs pause and resume; their pauses invalidate uninterrupted performance
comparisons. Synthetic button state is released without completing a pending click or drag on the next egui input frame.
Normal input resumes after the run. Viewport/DPR changes trigger layout settling and target resolution; they exclude the
run from fixed-geometry performance comparisons. Frame/action deadlines, missing/ambiguous targets, map failures, and
readiness timeouts fail the run. Content readiness does not prove compositor presentation. Remaining lifecycle and
performance acceptance belongs to the [activity-map plan](../plans/activity-map-workspace.md).
