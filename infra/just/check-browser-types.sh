#!/usr/bin/env bash
set -euo pipefail

compiler=$1
modules=$2
mkdir -p .tmp
workspace=$(mktemp -d .tmp/haos-types.XXXXXX)
trap 'rm -rf "$workspace"' EXIT
cp infra/javascript/*.mts infra/javascript/tsconfig.json "$workspace/"
ln -s "$modules" "$workspace/node_modules"
"$compiler" --project "$workspace/tsconfig.json"
