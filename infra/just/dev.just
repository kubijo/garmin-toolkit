import 'common.just'

# Enter the pinned development shell.
shell:
    just in-shell nix develop {{ flake }}

# Trace a development build without starting an application.
build-profile target label *options:
    @just in-shell nix develop {{ flake }} --max-jobs 1 --cores 2 --command uv run --frozen --project infra/python python infra/python/build_profile.py "$@"

# Profile the UI or desktop library's compiler internals in an isolated nightly cache.
compiler-profile target label *options:
    @just in-shell nix develop {{ flake }}\#compiler-profile --max-jobs 1 --cores 2 --command uv run --frozen --project infra/python python infra/python/build_profile.py "$@" --compiler-profile

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
