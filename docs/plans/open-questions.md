# Open questions

Resolve each under its owning plan, record the decision, and remove the question.

## OQ-022: Route-plan routing

Owner: [HASS watch workflow](hass-watch-vertical-slice.md). Select an existing road/trail routing engine and
cycling/running profiles behind an owned interface: embedded library, local sidecar, or replaceable service.
BRouter/GraphHopper are candidates; Rust alternatives lack equivalent evidenced sport policy. GPX, freehand, transforms,
FIT generation, and USB transfer do not depend on this choice.

## OQ-018: Authentication and encryption

Owner: [security](security-hardening.md). Select reviewed implementations independently for passwords, credential
custody, database protection, and encrypted snapshots. Decide target key store/manual unlock, database/host encryption,
and snapshot envelope. Require threat model, recovery, and independent cryptographic evidence. Preserve passwordless
use; create no cryptography. Blocks credential persistence, confidentiality claims, and unattended unlock.

## OQ-019: Sharing topology

Owner: [Connect and sharing](garmin-connect-and-sharing.md). Choose deployment-local ACLs, share bundles, peer sync, or
an optional coordinator. Start local without preventing peer exchange. Resolve identity, conflicts, and threat model
before shared database semantics.

## OQ-016: Release and publication

Owner: [device expansion](device-expansion-and-publication.md). Choose registries, signing/attestations, and
HASS/desktop channels: GitHub Releases/GHCR or mirrors of one signed build. A Homebrew cask can consume signed/notarized
macOS artifacts. GitHub Actions runs root Nix checks; Nix owns policy. [CLI distribution](distribution.md) owns Linux
archives, binstall, tap, and Flatpak feasibility.
