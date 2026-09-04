#!/usr/bin/env bash
set -euo pipefail

if (($# == 0)); then
    echo "usage: infra/hass-ssh.sh <command> [args...]" >&2
    exit 64
fi

ssh_config=${HASS_SSH_CONFIG:-"${HOME}/.ssh/config"}
ssh_identity=${HASS_SSH_IDENTITY:-"${HOME}/.ssh/id_rsa"}
ssh_target=${HASS_SSH_TARGET:-root@homeassistant.lan}

exec ssh \
    -F "${ssh_config}" \
    -o BatchMode=yes \
    -o ConnectTimeout=10 \
    -o IdentitiesOnly=yes \
    -i "${ssh_identity}" \
    "${ssh_target}" \
    "$@"
