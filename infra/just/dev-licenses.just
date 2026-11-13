import 'common.just'

# Verify generated dependency and asset license bundles.
check:
    just in-shell nix run {{ flake }}#licenses-check

# Regenerate committed dependency and asset license bundles.
generate *args:
    @just in-shell nix run {{ flake }}#licenses -- "$@"
