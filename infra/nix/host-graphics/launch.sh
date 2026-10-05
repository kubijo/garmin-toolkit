#!/usr/bin/env bash
set -euo pipefail

# Internal arguments supplied by the Nix wrapper, followed by the program and its arguments.
library_manifest=$1
cache_reader=$2
host_cache=$3
shift 3

# Desktop helpers must restore these values before launching host programs. Keep
# unset distinct from empty; the inner application wrapper also changes GIO/XDG.
for variable in LD_LIBRARY_PATH GIO_EXTRA_MODULES XDG_DATA_DIRS; do
    saved="NIX_APPIMAGE_HOST_$variable"
    export "${saved}_SET=${!variable+x}"
    export "$saved=${!variable-}"
done

declare -a library_directories=()
declare -A seen_directories=()

append_directory() {
    local directory=$1
    # Empty/relative entries would let the working directory supply libraries.
    if [[ $directory == /* && $directory != *:* && -d $directory && -z ${seen_directories["$directory"]+present} ]]; then
        seen_directories["$directory"]=1
        library_directories+=("$directory")
    fi
}

# Preserve the bundled ABI before exposing any host directories to the loader.
while IFS= read -r directory; do
    append_directory "$directory"
done <"$library_manifest"

if ((${#library_directories[@]} == 0)); then
    printf 'host graphics: bundled library directories are unavailable\n' >&2
    exit 1
fi

IFS=: read -r -a inherited_directories <<<"${LD_LIBRARY_PATH-}"
for directory in "${inherited_directories[@]}"; do
    append_directory "$directory"
done

export LD_LIBRARY_PATH
LD_LIBRARY_PATH=$(
    IFS=:
    echo "${library_directories[*]}"
)

# Read the host's cache with the bundled tool: never execute a host ldconfig or
# update the cache. It supplies multiarch and vendor directories without a
# distro-specific or GPU-specific list in the package.
if [[ -r $host_cache ]]; then
    if cache_contents=$("$cache_reader" -p -C "$host_cache"); then
        while IFS= read -r entry; do
            if [[ $entry == *' => /'* ]]; then
                library=${entry#* => }
                append_directory "${library%/*}"
            fi
        done <<<"$cache_contents"
    else
        printf 'host graphics: could not read %s; using standard library directories\n' "$host_cache" >&2
    fi
fi

# NixOS normally has no FHS linker cache; its driver link is the host interface.
# These are also the conventional fallback directories on hosts without a cache.
for directory in /run/opengl-driver/lib /lib /lib64 /usr/lib /usr/lib64; do
    append_directory "$directory"
done

LD_LIBRARY_PATH=$(
    IFS=:
    echo "${library_directories[*]}"
)
exec "$@"
