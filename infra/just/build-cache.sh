#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
temporary_root="$repository_root/.tmp"
shared="$temporary_root/cargo-target"
gallery_live="$temporary_root/gallery-target"
nightly="$temporary_root/build-profile/nightly-target"
legacy=(
    "$temporary_root/nix-cargo-target"
    "$temporary_root/gallery-check-target"
    "$temporary_root/build-profile/target"
    "$temporary_root/hass-native-target"
    "$temporary_root/hass-web-target"
)

size_human() {
    local path=$1
    if [[ -e $path ]]; then
        du -sh "$path" | cut -f1
    else
        printf '0\n'
    fi
}

report_path() {
    printf '%8s  %s\n' "$(size_human "$2")" "$1"
}

case ${1:-} in
report)
    if (($# != 1)); then
        printf 'usage: build-cache.sh report\n' >&2
        exit 64
    fi
    report_path shared "$shared"
    report_path gallery-live "$gallery_live"
    report_path nightly "$nightly"
    for path in "${legacy[@]}"; do
        report_path "legacy: ${path#"$temporary_root"/}" "$path"
    done
    report_path 'all .tmp' "$temporary_root"
    report_path 'Cargo home (read-only)' "$HOME/.cargo"
    report_path 'Nix user cache (read-only)' "$HOME/.cache/nix"
    ;;
clean)
    if (($# != 3)); then
        printf 'usage: build-cache.sh clean {shared|gallery-live|nightly|legacy} {dry-run|apply}\n' >&2
        exit 64
    fi
    case $2 in
    shared) targets=("$shared") ;;
    gallery-live) targets=("$gallery_live") ;;
    nightly) targets=("$nightly") ;;
    legacy) targets=("${legacy[@]}") ;;
    *)
        printf 'unknown cache: %s\n' "$2" >&2
        exit 64
        ;;
    esac
    case $3 in
    dry-run | apply) ;;
    *)
        printf 'unknown mode: %s\n' "$3" >&2
        exit 64
        ;;
    esac
    if [[ -L $temporary_root || -L $temporary_root/build-profile ]]; then
        printf 'refusing symlinked cache parent under %s\n' "$temporary_root" >&2
        exit 1
    fi
    for path in "${targets[@]}"; do
        if [[ -L $path || (-e $path && ! -d $path) ]]; then
            printf 'refusing non-directory cache path: %s\n' "$path" >&2
            exit 1
        fi
    done
    for path in "${targets[@]}"; do
        printf '%s: %s  %s\n' "$3" "$(size_human "$path")" "$path"
        if [[ $3 == apply && -d $path ]]; then
            rm -rf "$path"
        fi
    done
    ;;
*)
    printf 'usage: build-cache.sh {report|clean}\n' >&2
    exit 64
    ;;
esac
