import 'common.just'

# Validate extracted and translated ICU messages.
check:
    just in-shell nix run {{ flake }}#i18n-check

# Update the Czech catalog from source messages.
sync:
    just in-shell nix run {{ flake }}#i18n-sync
