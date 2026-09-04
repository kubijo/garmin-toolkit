import 'common.just'

# Fail fast on formatting and repository policy without compiling the workspace.
preflight:
    just in-shell nix run {{ flake }}#validate

# Run the complete local release gate, cheap checks first.
full:
    just in-shell nix run {{ flake }}#validate
    just in-shell nix run {{ flake }}#project-lint

# Evaluate and build every pure Nix flake check.
check:
    just in-shell nix flake check {{ flake }}

# Check dependency advisories and Git secrets.
audit *args:
    @just in-shell nix run {{ flake }}#audit -- "$@"

# Run the complete Rust test suite.
test:
    just in-shell nix run {{ flake }}#test

# Run repository policy before the complete Rust and gallery lint suite.
lint:
    just in-shell nix run {{ flake }}#lint
    just in-shell nix run {{ flake }}#project-lint

# Format every maintained source type.
format:
    just in-shell nix run {{ flake }}#format

# Measure workspace line coverage and enforce the configured floor.
coverage *args:
    @just in-shell nix run {{ flake }}#coverage -- "$@"

# Validate extracted and translated ICU messages.
i18n:
    just in-shell nix run {{ flake }}#i18n-check

# Verify generated dependency and asset license bundles.
licenses:
    just in-shell nix run {{ flake }}#licenses-check

# Verify committed SQLx metadata against every static query.
sqlx:
    just in-shell nix run {{ flake }}#sqlx-check
