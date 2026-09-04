# Open questions

## Usage

Questions belong to their earliest resolving plan. Resolve one by recording the decision and removing it here, or
explicitly reassign it. This file contains only live uncertainty.

## Home Assistant watch vertical slice

### OQ-022: Route-plan routing

- **Question:** Which existing engine and datasets calculate cycling and running geometry that follows roads and trails?
- **Current leaning:** Keep routing behind an owned interface and implement no routing algorithm. BRouter and
  GraphHopper are mature JVM candidates; current Rust candidates lack equivalent evidenced sport policy.
- **Initial surface:** One cycling and one running profile with routed, reorderable, and draggable control points.
- **Options:** Embedded library, local sidecar, or replaceable remote service.
- **Blocks:** Routed geometry only. GPX, freehand plans, explicit transforms, FIT generation, and USB deployment do not
  depend on this decision.

## Security hardening

### OQ-018: Authentication and encryption

- **Question:** Which reviewed implementations provide passwords, credential custody, database protection, and encrypted
  snapshots?
- **Current direction:** Keep trusted passwordless operation; assess each concern independently and build no
  cryptography.
- **Options:** Target key stores or manual unlock; encrypted database or host storage; an audited snapshot envelope.
- **Resolution:** Each feature needs a threat model, recovery design, and public independent evidence for its
  cryptography.
- **Blocks:** Password claims, persisted connector credentials, encrypted storage/snapshots, and unattended unlock.

## Garmin Connect and sharing

### OQ-019: Sharing topology

- **Question:** Is sharing deployment-local or cross-deployment without a mandatory service?
- **Current leaning:** Local first without blocking later peer exchange.
- **Options:** Local ACLs, share bundles, peer sync, or optional coordinator.
- **Blocks:** Shared database semantics, identity, conflict handling, and threat model.

## Device expansion and publication

### OQ-016: Release and publication

- **Owner:** [Device expansion and publication](device-expansion-and-publication.md).
- **Question:** Which registries, signing, attestations, and channels publish HASS/desktop artifacts?
- **Existing direction:** GitHub Actions runs root `nix flake check`; Nix owns policy.
- **Options:** GitHub Releases+GHCR, another registry, or mirrors from one signed build. A project-owned Homebrew cask
  may consume the signed and notarized macOS release artifact.
- **Blocks:** Image publication, desktop artifacts, provenance attestations, and release documentation.
- **Related:** [CLI distribution](distribution.md) owns archives, binstall, a Linux tap, and Flatpak feasibility.
