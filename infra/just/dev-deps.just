import 'common.just'

# Report outdated Nix inputs, Python and JavaScript dependencies, and CI actions.
outdated *args:
    @just in-shell nix run {{ flake }}#outdated -- "$@"

# Update Rust dependencies and Nix flake inputs.
update:
    cargo update
    nix flake update
