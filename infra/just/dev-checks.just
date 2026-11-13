import 'common.just'

# Lint the actual browser target using the pinned Nix WASM toolchain.
wasm:
    just in-shell nix run {{ flake }}#wasm-lint

# Run the gallery gate with pinned native and headless dependencies.
gallery:
    just in-shell nix run {{ flake }}#gallery-check

# Evaluate and build every pure Nix flake check.
nix *args:
    @just in-shell nix flake check {{ flake }} --no-update-lock-file "$@"

# Check dependency advisories and Git secrets.
audit *args:
    @just in-shell nix run {{ flake }}#audit -- "$@"

# Check Data Package compatibility and build its standalone distribution archive.
datapackage:
    just in-shell nix run {{ flake }}#datapackage-check

# Check bindgen cache restoration and invalidation with a real WASM fixture.
trunk-cache:
    just in-shell nix develop {{ flake }} --command uv run --frozen --project infra/python python infra/python/trunk_cache_acceptance.py
