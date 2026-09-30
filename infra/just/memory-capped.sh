#!/usr/bin/env bash
set -euo pipefail

if (($# == 0)); then
    printf 'usage: memory-capped COMMAND [ARGUMENT ...]\n' >&2
    exit 64
fi

if [[ $(uname -s) == Darwin ]]; then
    export CARGO_BUILD_JOBS=1
    export NIX_BUILD_CORES=1
    export NIX_CONFIG="${NIX_CONFIG:+${NIX_CONFIG}$'\n'}max-jobs = 1"$'\n''cores = 1'
    exec "$@"
fi

# Cargo otherwise uses every logical CPU, which can make rustdoc workers thrash
# against MemoryHigh before reaching the hard memory limit.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

if ! command -v systemd-run >/dev/null 2>&1 \
    || ! command -v systemctl >/dev/null 2>&1 \
    || ! systemctl --user show-environment >/dev/null 2>&1; then
    exec "$@"
fi

exec systemd-run --user --scope --quiet \
    -p MemoryHigh=4G \
    -p MemoryMax=6G \
    -p MemorySwapMax=1G \
    "$@"
