repository_root := parent_directory(parent_directory(source_directory()))
flake := "git+file://" + repository_root
export GARMIN_TOOLKIT_CACHE_DIR := repository_root + "/.tmp/app-cache"

# Target-specific flags keep the WASM and macOS linkers independent of Linux development.
linux_link_flags := "-C link-arg=-fuse-ld=mold"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS := "-C linker-features=-lld " + linux_link_flags
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS := linux_link_flags

set default-list
set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# Fail before native compilation when the selected linker is unavailable.
[no-exit-message]
[private]
[script("bash")]
require-linker:
    if [[ {{ os() }} == linux ]] \
        && { ! command -v mold >/dev/null 2>&1 || ! command -v ld.mold >/dev/null 2>&1; }; then
        printf 'error: Linux development builds require mold and ld.mold on PATH. Run `nix develop` or install Mold.\n' >&2
        exit 1
    fi

# Run through the ambient toolchain when available, otherwise enter the Nix shell.
[private]
[script("bash")]
in-shell +cmd:
    set -euo pipefail

    cd {{ quote(repository_root) }}

    if command -v python3 >/dev/null 2>&1; then
        python3 infra/python/prepare_cargo_target.py
    fi

    if command -v "$1" >/dev/null 2>&1 \
        && { [[ "$1" != cargo || $(uname -s) != Linux ]] \
            || { command -v mold >/dev/null 2>&1 && command -v ld.mold >/dev/null 2>&1; }; }; then
        exec "$@"
    fi
    exec nix develop {{ quote(flake) }} --command "$@"
