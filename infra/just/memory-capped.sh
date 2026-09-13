#!/usr/bin/env bash
set -euo pipefail

if (($# == 0)); then
    printf 'usage: memory-capped COMMAND [ARGUMENT ...]\n' >&2
    exit 64
fi

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
