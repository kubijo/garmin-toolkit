#!/usr/bin/env bash
# Bootstrap only disposable GitHub-hosted Linux runners; never change a developer's daemon.
set -euo pipefail

case "${1:-}" in
prepare | limits)
    [[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted && ${RUNNER_OS:-} == Linux ]] || {
        printf 'This operation requires a disposable GitHub-hosted Linux runner.\n' >&2
        exit 1
    }
    ;;
esac

case "${1:-}" in
prepare)
    if [[ -n ${TMPDIR:-} ]]; then mkdir -p -- "$TMPDIR"; fi
    case "${2:-false}" in
    true)
        test -c /dev/kvm
        sudo chmod a+rw /dev/kvm
        sudo loginctl enable-linger "$(id -un)"
        sudo systemctl start "user@$(id -u).service"
        XDG_RUNTIME_DIR="/run/user/$(id -u)"
        export XDG_RUNTIME_DIR
        systemctl --user show-environment >/dev/null
        printf 'XDG_RUNTIME_DIR=%s\n' "$XDG_RUNTIME_DIR" >>"$GITHUB_ENV"
        ;;
    false) ;;
    *) exit 64 ;;
    esac
    case "${3:-false}" in
    true)
        sudo apt-get update
        sudo apt-get install -y --no-install-recommends qemu-system-x86 qemu-utils ovmf mtools parted
        ;;
    false) ;;
    *) exit 64 ;;
    esac
    ;;
limits)
    sudo systemctl set-property --runtime nix-daemon.service MemoryMax=6G CPUQuota=200%
    test "$(systemctl show nix-daemon.service --property MemoryMax --value)" = 6442450944
    test "$(systemctl show nix-daemon.service --property CPUQuotaPerSecUSec --value)" = 2s
    ;;
checks)
    case "${2:-}" in
    rust) nix build -L .#checks.x86_64-linux.rust-source-closure --max-jobs 1 --cores 2 --no-update-lock-file ;;
    tooling) nix flake check --no-build --no-update-lock-file ;;
    desktop | gallery | hass) ;;
    *) exit 64 ;;
    esac
    nix build -L ".#legacyPackages.x86_64-linux.ci.$2" --max-jobs 1 --cores 2 --no-update-lock-file
    ;;
require-success)
    shift
    test "$#" -gt 0
    for result in "$@"; do test "$result" = success; done
    ;;
cleanup)
    # Recovery uses GH_TOKEN; remove registry credentials even if Nix cannot start.
    rm -f -- .tmp/docker-auth/config.json
    exec nix run .#hass-release -- cleanup-staging --directory .tmp/publication
    ;;
*)
    printf 'usage: runner.sh {prepare [KVM HAOS]|limits|checks GROUP|require-success RESULT...}\n' >&2
    exit 64
    ;;
esac
