import 'common.just'

# Verify committed SQLx metadata against every static query.
check:
    just in-shell nix run {{ flake }}#sqlx-check

# Regenerate SQLx's checked-in offline query metadata.
prepare:
    just in-shell nix run {{ flake }}#sqlx-prepare
