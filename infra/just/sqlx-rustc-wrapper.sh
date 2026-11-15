#!/usr/bin/env bash
set -euo pipefail

# cargo sqlx prepare forces online mode globally.
# Only workspace packages should describe queries against
# our database; dependencies own their query metadata.
# The prepare command selects every workspace member,
# making each one primary.
if [[ ${CARGO_PRIMARY_PACKAGE:-} != 1 ]]; then
    export SQLX_OFFLINE=true
    unset SQLX_OFFLINE_DIR SQLX_TMP DATABASE_URL
fi

if [[ -n ${GARMIN_SQLX_RUSTC_WRAPPER:-} ]]; then
    exec "$GARMIN_SQLX_RUSTC_WRAPPER" "$@"
fi
exec "$@"
