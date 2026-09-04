# 0005: Nix-owned quality gate

## Decision

Pin tools and hermetic checks in the root flake with one lock. Just exposes `format`, `lint`, and `validate`. CI
installs Nix and runs `nix flake check -L --keep-going --no-update-lock-file`. Check local links, not remote
availability.

## Why

Pinned Nix avoids tool drift and forge-owned policy. `validate` remains a visible local composition.

## Consequences

Root checks own hermetic gates; `lint` owns fast local policy; network audits and reports remain explicit apps. See
[flake.nix](../../flake.nix), [justfile](../../justfile), and [CI](../../.github/workflows/ci.yml).
