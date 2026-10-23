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
    runner_kvm=${2:-false}
    runner_haos=${3:-false}
    case "$runner_kvm" in true | false) ;; *) exit 64 ;; esac
    case "$runner_haos" in false | true | x86_64-linux | aarch64-linux) ;; *) exit 64 ;; esac
    if [[ $runner_kvm == true ]]; then
        test -c /dev/kvm
        sudo chmod a+rw /dev/kvm
    fi
    if [[ $runner_kvm == true || $runner_haos != false ]]; then
        sudo loginctl enable-linger "$(id -un)"
        sudo systemctl start "user@$(id -u).service"
        XDG_RUNTIME_DIR="/run/user/$(id -u)"
        export XDG_RUNTIME_DIR
        systemctl --user show-environment >/dev/null
        printf 'XDG_RUNTIME_DIR=%s\n' "$XDG_RUNTIME_DIR" >>"$GITHUB_ENV"
    fi
    case "$runner_haos" in
    true | x86_64-linux)
        sudo apt-get update
        sudo apt-get install -y --no-install-recommends qemu-system-x86 qemu-utils ovmf mtools parted
        ;;
    aarch64-linux)
        sudo apt-get update
        sudo apt-get install -y --no-install-recommends \
            qemu-system-arm qemu-efi-aarch64 qemu-utils \
            ipxe-qemu ipxe-qemu-256k-compat-efi-roms seabios mtools parted
        ;;
    false) ;;
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
    cleanup_status=0
    rm -f -- .tmp/docker-auth/config.json || cleanup_status=$?
    nix run .#hass-ci -- cleanup || cleanup_status=$?
    exit "$cleanup_status"
    ;;
recovery-cleanup)
    cleanup_status=0
    rm -f -- .tmp/recovery-auth/config.json || cleanup_status=$?
    nix run .#hass-recovery -- cleanup || cleanup_status=$?
    exit "$cleanup_status"
    ;;
*)
    printf 'usage: runner.sh {prepare [KVM HAOS]|limits|checks GROUP|require-success RESULT...|cleanup|recovery-cleanup}\n' >&2
    exit 64
    ;;
esac
