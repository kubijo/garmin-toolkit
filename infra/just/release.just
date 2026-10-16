import 'common.just'

[private]
in-release +args:
    @just in-shell nix develop {{ flake }} \
        --max-jobs 1 \
        --cores 2 \
        --command uv run \
        --frozen \
        --project infra/python \
        python infra/python/hass_release.py "$@"

# Assemble a local HASS release candidate from a clean source tag and existing build artifacts.
assemble *options:
    @just release::in-release assemble "$@"

# Verify local candidate content against an independently expected manifest hash and source identity.
verify *options:
    @just release::in-release verify "$@"
