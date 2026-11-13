import 'common.just'

# Focused repository and platform checks.
mod checks 'dev-checks.just'

# Build-cache inspection and cleanup.
mod cache 'dev-cache.just'

# Build and compiler profiling.
mod profile 'dev-profile.just'

# Coverage collection and ownership reports.
mod coverage 'dev-coverage.just'

# Dependency reports and updates.
mod deps 'dev-deps.just'

# Translation checks and catalog synchronization.
mod i18n 'dev-i18n.just'

# License-bundle checks and generation.
mod licenses 'dev-licenses.just'

# SQLx offline metadata checks and preparation.
mod sqlx 'dev-sqlx.just'

# Enter the pinned development shell.
shell:
    just in-shell nix develop {{ flake }}

# Format every maintained source type.
format:
    just in-shell nix run {{ flake }}#format

# Fail fast on formatting and repository policy without compiling the workspace.
preflight:
    just in-shell nix run {{ flake }}#validate

# Run the complete local release gate, cheap checks first.
verify:
    just in-shell nix run {{ flake }}#validate
    just in-shell nix run {{ flake }}#project-lint

# Run the complete Rust test suite.
test:
    just in-shell nix run {{ flake }}#test

# Build API documentation with warnings denied.
docs *args:
    @just in-shell nix run {{ flake }}#docs -- "$@"
