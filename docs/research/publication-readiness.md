# Publication readiness

## Published AMD64 release

The first public experimental release is [v0.1.1](https://github.com/kubijo/garmin-toolkit/releases/tag/v0.1.1),
published on 2026-10-20 from commit `6f300c9`. The
[tagged candidate run](https://github.com/kubijo/garmin-toolkit/actions/runs/39377755631) passed native image/source
assembly, HAOS acceptance, signed provenance, and independent draft verification. The
[promotion run](https://github.com/kubijo/garmin-toolkit/actions/runs/39390440018) passed promotion and public
repository installation. The release includes signed inventories, dependency sources, acceptance reports, runtime
versions, and dark app captures.

Qualification covers AMD64. ARM64 runtime acceptance and publication failure recovery remain in the
[publication plan](../plans/device-expansion-and-publication.md#hass-experimental-release). Published artifacts retain
their original source and dependency versions; subsequent source changes require a new release to reach installed apps.

## Withdrawal acceptance

The [withdrawal check](../../infra/integration/hass/README.md#withdrawal-and-replacement) passed on AMD64 with HAOS
18.3, Supervisor 2026.10.3, and Core 2026.10.2, using the authenticated published v0.1.2 baseline and the
[dependency-update CI image](https://github.com/kubijo/garmin-toolkit/actions/runs/39745795968) under private Supervisor
version `0.1.3`; the candidate's OCI bytes remain unchanged.

Store refresh hid the app while its installed image, running state, populated profile/route/Course, and exact downloads
survived. Replacement preserved them through restart, HAOS reboot, and cold restore that reversed post-backup mutations.
Matching withdrawal retries preserved both refs; stale promotions failed before and after replacement qualification. All
owned scopes, listeners, browser, and VM scratch were removed.

This covers Supervisor and local Git; [public GitHub/GHCR recovery](../plans/device-expansion-and-publication.md) and
[missing header/sidebar captures](../plans/shared-interface-workflows.md#runtime-acceptance) remain open.

## Dependency sources

The [inventory](../../infra/release/hass-files.toml) includes workspace source, Cargo/asset notices, and image-bound
dependency sources, including inherited GVfs dependencies despite raw MTP. SBOM `NOASSERTION` records do not establish
redistribution coverage; the build-only OCI writer is not shipped.

Distribute sources, notices, build instructions, checksums, and attestations through GitHub Releases. Verify downloaded
bundles against the release's signed inventory before publication.

[AMD64 audit](https://github.com/kubijo/garmin-toolkit/actions/runs/39208332753), commit `7887023`, verified six
downloaded parts, 10,905 source roots, 6,432 recipes, and exact coverage of the image's 232 runtime outputs. The
inventory reports no unresolved runtime licenses. Native/WASM dependencies, Cargo/asset notices, and the lockfile
matched the native receipt and committed source. The archive contains 1,115,421 entries and 21,063,075,840 decoded tar
bytes. These measurements describe the pre-release audit at `7887023`; the published release has its own signed
inventory and acceptance evidence.

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

HASS uses GHCR and GitHub Releases for AMD64 experimental releases. ARM64 requires runtime acceptance. The
[README](../../README.md#home-assistant) and [quickstart](../../apps/garmin-hass/addon/DOCS.md#install-and-open) provide
My Home Assistant links for `https://github.com/kubijo/garmin-toolkit#hass-apps`. Users confirm repository addition and
installation. GitHub is the only project hosting.

Desktop publication still needs supported-host, channel, and signing decisions; mobile remains separate.
