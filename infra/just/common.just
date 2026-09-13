repository_root := parent_directory(parent_directory(source_directory()))
flake := "git+file://" + repository_root
memory_capped := repository_root / "infra/just/memory-capped.sh"
export GARMIN_TOOLKIT_CACHE_DIR := repository_root + "/.tmp/app-cache"

set default-list
set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# Run through the ambient toolchain when available, otherwise enter the Nix shell.
[private]
in-shell +cmd:
    #!/usr/bin/env bash
    set -euo pipefail

    cd {{ quote(repository_root) }}

    if command -v "$1" >/dev/null 2>&1; then
        exec {{ quote(memory_capped) }} env CARGO_INCREMENTAL=0 "$@"
    fi
    exec {{ quote(memory_capped) }} env CARGO_INCREMENTAL=0 \
        nix develop {{ quote(flake) }} --command "$@"
