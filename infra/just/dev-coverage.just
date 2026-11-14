import 'common.just'

# Measure workspace line coverage and enforce the configured floor.
run *args:
    @just in-shell nix run {{ flake }}#coverage -- "$@"

# Summarize an existing LCOV report by Cargo owner without changing coverage gates.
owners +args:
    @just in-shell uv run --frozen --project infra/python python infra/python/coverage_owners.py "$@"
