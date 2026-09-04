# Build system

The root flake pins Rust 1.98, dependencies, checks, and deliverables. App flakes own target packaging; the root
supplies filtered workspace source and re-exports their outputs.

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

Connect IQ remains a future, local-only integration under ADR 0009.
