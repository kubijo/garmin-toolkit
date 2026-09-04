# 0032: Linux desktop packaging

## Decision

Target production and mock AppImage and Flatpak builds on Linux `x86_64` and `aarch64`. `x86_64` is build-tested;
`aarch64` remains evaluation-only until built on that architecture. Keep macOS testable but outside the initial
packaging gate; omit 32-bit targets.

AppImage uses a pinned compatibility toolchain and type-2 runtime. Flatpak builds offline from Nix-vendored Cargo
sources on Freedesktop 25.08. Both consume the same generated launcher and brand assets.

## Why

AppImage provides a portable file; Flatpak provides a sandboxed installation. Production and mock identities must not
share platform data.

## Consequences

Linux changes must build both formats and modes. Flatpak building requires the matching Platform, SDK, and Rust
extension. Runtime or compatibility-pin updates require rebuilding and testing the artifacts on supported systems.
