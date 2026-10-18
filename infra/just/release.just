import 'common.just'

[private]
in-release +args:
    @just in-shell nix run {{ flake }}#hass-release -- "$@"

# Assemble a local HASS release candidate from a clean source tag and existing build artifacts.
assemble *options:
    @just release::in-release assemble "$@"

# Verify local candidate content against an independently expected manifest hash and source identity.
verify *options:
    @just release::in-release verify "$@"

# Verify signed checksum provenance before release files and registry image attestations.
verify-provenance *options:
    @just release::in-release verify-provenance "$@"

# Record existing native Nix image/SBOM outputs for the publication workflow.
record-build *options:
    @just release::in-release record-build "$@"

# Bundle exact dependency sources and report runtime license coverage.
bundle-sources *options:
    @just release::in-release bundle-sources "$@"

# Generate a release-only Home Assistant app repository for selected architectures.
repository *options:
    @just release::in-release repository "$@"

# Prepare annotated OCI manifests from verified native image exports; no registry access.
prepare-registry *options:
    @just release::in-release prepare-registry "$@"

# Pull an immutable registry index and verify every platform against native exports.
verify-registry *options:
    @just release::in-release verify-registry "$@"
