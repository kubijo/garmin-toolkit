# CLI distribution

Publish Linux CLI archives; HASS/desktop publication belongs to [device expansion](device-expansion-and-publication.md).
Flathub is excluded. Self-hosted Flatpak remains experimental.

- **Archives:** Build x86_64/aarch64 GNU Linux from one clean tag with licenses, checksums, attestations, install notes.
  Test help, simulator, discovery, network, raw MTP/GVfs outside Nix on Ubuntu/Fedora. Audit minimum glibc, GLib, GVfs,
  udev, USB, certificates and linkage before choosing builder.
- **Binstall:** Assess required published crate closure; inspect package archives. Verify both architectures with
  compilation fallback disabled, or reject. Do not publish internal crates solely for binstall.
- **Homebrew:** Project-owned Linux tap; derive dependencies from linkage audit. Test install, upgrade, removal, both
  architectures, and non-hardware behavior.
- **Flatpak:** Test network, GVfs, mass storage and USB portal on GNOME/KDE. No `--device=all`. Self-host only after
  hardware discovery/read/write/cancel/recovery pass, otherwise reject.

Evaluate pinned [cargo-dist](https://axodotdev.github.io/cargo-dist/book/); defer musl until the native stack is
portable. README lists verified channels only.

References: [Cargo packaging](https://doc.rust-lang.org/cargo/commands/cargo-package.html),
[binstall](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md),
[Homebrew taps](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap),
[Flatpak permissions](https://docs.flatpak.org/en/latest/sandbox-permissions.html),
[hosting](https://docs.flatpak.org/en/latest/hosting-a-repository.html).
