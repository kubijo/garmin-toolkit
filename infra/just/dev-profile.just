import 'common.just'

# Trace a development build without starting an application.
build target label *options:
    @just in-shell nix develop {{ flake }} --command uv run --frozen --project infra/python python infra/python/build_profile.py "$@"

# Profile the UI or desktop library's compiler internals in an isolated nightly cache.
compiler target label *options:
    @just in-shell nix develop {{ flake }}\#compiler-profile --command uv run --frozen --project infra/python python infra/python/build_profile.py "$@" --compiler-profile
