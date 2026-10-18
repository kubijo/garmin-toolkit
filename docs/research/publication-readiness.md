# Publication readiness

## Dependency sources

The [inventory](../../infra/release/hass-files.toml) includes workspace source, Cargo/asset notices, and image-bound
dependency sources, including inherited GVfs dependencies despite raw MTP. SBOM `NOASSERTION` records do not establish
redistribution coverage; the build-only OCI writer is not shipped.

Distribute sources, notices, build instructions, checksums, and attestations through GitHub Releases. Verify downloaded
bundles before clearing the
[publication gate](../plans/device-expansion-and-publication.md#provenance-and-redistribution).

[AMD64 audit](https://github.com/kubijo/garmin-toolkit/actions/runs/39208332753), commit `7887023`, verified six
downloaded parts, 10,905 source roots, 6,432 recipes, and exact coverage of the image's 232 runtime outputs. The
inventory reports no unresolved runtime licenses. Native/WASM dependencies, Cargo/asset notices, and the lockfile
matched the native receipt and committed source. The archive contains 1,115,421 entries and 21,063,075,840 decoded tar
bytes. Tagged publication and live provenance remain unqualified.

Source stream SHA-256: `3d584f3a5352333381b3e2162719456b6ebd730b80cf930b3493534b3d3767ff`. Image SHA-256:
`100641a9ac45978967cb3fa7bc3ec48365b7bb129b1ac35ad6d306c423e10e40`.

ICU 78.3's metadata-only license correction is guarded by its version and source SHA-256
`3a2e7a47604ba702f345878308e6fefeca612ee895cf4a5f222e7955fabfe0c0`. Its
[complete notice](https://github.com/unicode-org/icu/blob/release-78.3/LICENSE) declares Unicode-3.0 and additional
third-party terms; preserve it inside the original source archive. The notice SHA-256 is
`e55522d81edc687a341a4411e0776e54ca654e90147f354a90458aaced4116af`.

Nix labels DejaVu 2.37 and libgd 2.3.3 `LicenseRef-nixos-free`. Their bundled notices match the versioned upstream
copies:

- [DejaVu LICENSE](https://github.com/dejavu-fonts/dejavu-fonts/blob/version_2_37/LICENSE):
  `7a083b136e64d064794c3419751e5c7dd10d2f64c108fe5ba161eae5e5958a93`.
- [libgd COPYING](https://github.com/libgd/libgd/blob/gd-2.3.3/COPYING):
  `005f4b6b0141d1bd11d371bbf7d4f67947f85a4906b7f5465f942204cf918ba3`.

Retain the complete notices and their copyright, attribution, and font naming conditions with distributed artifacts.

## Distribution choices

HASS uses GHCR and GitHub Releases for AMD64 experimental releases. ARM64 requires runtime acceptance. Use GitHub as the
only project hosting.

Desktop publication still needs supported-host, channel, and signing decisions; mobile remains separate.
