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
