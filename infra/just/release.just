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

# Record existing native Nix image/SBOM outputs for the publication workflow.
record-build *options:
    @just release::in-release record-build "$@"

# Generate a release-only Home Assistant app repository for selected architectures.
repository *options:
    @just release::in-release repository "$@"
