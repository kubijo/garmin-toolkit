# CLI distribution channels

Build verified Linux CLI archives for GitHub. The current distribution direction excludes Flathub for this TUI; its
console/host workflow needs USB or GVfs access. Keep self-hosted Flatpak experimental.

This plan owns CLI archives, binstall, the Homebrew tap, and the Flatpak feasibility check.
[Device expansion and publication](device-expansion-and-publication.md) owns HASS/desktop publication and support
claims. [OQ-016](open-questions.md#oq-016-release-and-publication) records its release choices. Reuse release tooling
across channels without duplicating decisions.

## Investigation

1. Build `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` archives from a clean tag. Include the binary,
   licenses, checksum, attestation, and concise install notes.
2. Test outside Nix on clean Ubuntu and Fedora: help, simulator, discovery, network, raw MTP, and GVfs.
3. Audit native linkage and minimum glibc, GLib, GVfs, udev, USB, and certificate requirements before choosing a release
   builder. Evaluate pinned, reviewable `dist` output. Defer musl until the native stack proves portable.
4. For `cargo-binstall`, decide whether publishing the required crate closure is worthwhile. Inspect every
   `cargo package` archive and test prebuilt installation with compilation fallback disabled. Do not publish internal
   crates solely to support binstall.
5. For Homebrew, start with a Linux-only project tap. Derive dependencies from the linkage audit; test source or binary
   installation, upgrades, removal, both architectures, and real non-hardware behavior. Consider core only after stable
   releases and users.
6. For Flatpak, test network, GVfs, mass storage, and the USB portal on GNOME and KDE. Reject `--device=all`. Self-host
   only if discovery, read, write, cancellation, and recovery work on hardware.

## Exit criteria

- One tag reproducibly produces both archives, checksums, and attestations.
- Archives run outside Nix with documented native dependencies.
- Binstall uses upstream archives on both architectures or is explicitly rejected.
- The tap installs, tests, upgrades, and removes cleanly.
- Flatpak is hardware-verified and self-hosted or rejected.
- README lists only channels that pass these gates.

## References

- [Cargo package rules](https://doc.rust-lang.org/cargo/commands/cargo-package.html)
- [cargo-binstall publishers](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md)
- [Homebrew](https://docs.brew.sh/Adding-Software-to-Homebrew)
  - [Homebrew taps](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
  - [Homebrew acceptance](https://docs.brew.sh/Acceptable-Formulae)
- [`dist` and Homebrew](https://axodotdev.github.io/cargo-dist/book/installers/homebrew.html)
- [Flatpak permissions](https://docs.flatpak.org/en/latest/sandbox-permissions.html)
- [Flatpak hosting](https://docs.flatpak.org/en/latest/hosting-a-repository.html)
- [Flathub requirements](https://docs.flathub.org/docs/for-app-authors/requirements)
