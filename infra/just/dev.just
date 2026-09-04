import 'common.just'

# Enter the pinned development shell.
shell:
    just in-shell nix develop {{ flake }}

# Build the production garmin-cli package.
build:
    just in-shell nix build {{ flake }}#garmin-cli

# Build API documentation with warnings denied.
docs *args:
    @just in-shell nix run {{ flake }}#docs -- "$@"

# Report outdated direct Rust dependencies without changing manifests.
outdated *args:
    @just in-shell nix run {{ flake }}#outdated -- "$@"

# Update the Czech catalog from source messages.
i18n-sync:
    just in-shell nix run {{ flake }}#i18n-sync

# Regenerate committed dependency and asset license bundles.
licenses *args:
    @just in-shell nix run {{ flake }}#licenses -- "$@"

# Regenerate SQLx's checked-in offline query metadata.
sqlx-prepare:
    just in-shell nix run {{ flake }}#sqlx-prepare
