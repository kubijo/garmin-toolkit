#!/usr/bin/env bash
set -euo pipefail

grammar=$1
if [[ $# -gt 2 || ($# -eq 2 && $2 != --install) ]]; then
    printf 'Usage: nix run .#zed-monkey-c -- [--install]\n' >&2
    exit 2
fi
extension=infra/zed/monkey-c
target_dir="$PWD/.tmp/zed-monkey-c-target"
output=dist/zed-monkey-c

RUSTC_WRAPPER='' CARGO_BUILD_JOBS=2 cargo build --locked \
    --manifest-path "$extension/Cargo.toml" --target wasm32-wasip2 --target-dir "$target_dir"
mkdir -p "$output/grammars"
cp "$target_dir/wasm32-wasip2/debug/garmin_toolkit_monkey_c_zed.wasm" "$output/extension.wasm"
cp "$extension/extension.toml" "$output/extension.toml"
cp -R "$extension/languages" "$output/"
install -m 644 "$grammar/monkey_c.wasm" "$output/grammars/monkey_c.wasm"
printf 'Zed extension ready: %s/%s\n' "$PWD" "$output"

if [[ ${2:-} == --install ]]; then
    extensions="${XDG_DATA_HOME:-$HOME/.local/share}/zed/extensions"
    destination="$extensions/installed/garmin-toolkit-monkey-c"
    python -c '
import pathlib, sys, tomllib
path = pathlib.Path(sys.argv[1])
if path.is_symlink() or (path.exists() and tomllib.loads((path / "extension.toml").read_text()).get("id") != "garmin-toolkit-monkey-c"):
    raise SystemExit("Refusing to replace another extension or a dev-extension symlink")
' "$destination"
    mkdir -p "$extensions/installed"
    staging=$(mktemp -d "$extensions/.monkey-c.XXXXXX")
    previous="$staging.previous"
    cleanup() {
        if [[ -d $previous && ! -e $destination ]]; then
            mv "$previous" "$destination"
        fi
        rm -rf "$staging"
    }
    trap cleanup EXIT
    cp -R "$output/." "$staging/"
    if [[ -d $destination ]]; then
        mv "$destination" "$previous"
    fi
    # Replacing the directory entry also makes Zed notice updates to an existing extension.
    mv "$staging" "$destination"
    rm -rf "$previous"
    printf 'Installed: %s\n' "$destination"
fi
