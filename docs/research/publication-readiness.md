# HASS publication qualification

Installation belongs to the [quickstart](../../apps/garmin-hass/addon/DOCS.md); publication, verification, and recovery
procedures belong to the [release pipeline](../../infra/release/README.md).

## Published AMD64/ARM64 release

Signed [v0.1.5](https://github.com/kubijo/garmin-toolkit/releases/tag/v0.1.5), source `28a1bb2`, qualified both
architectures. The [candidate run](https://github.com/kubijo/garmin-toolkit/actions/runs/40118061032) verified native
images/sources and staged lifecycle behavior, rejected invalid provenance, and independently authenticated draft
downloads. The [promotion run](https://github.com/kubijo/garmin-toolkit/actions/runs/40178263044) passed public
installation, populated ingress, restart, HAOS reboot, cold restore, exact downloads, and cleanup. Anonymous GHCR pulls
matched the signed index.

Lifecycle checks upgraded authenticated published [v0.1.1](https://github.com/kubijo/garmin-toolkit/releases/tag/v0.1.1)
on AMD64 and [v0.1.3](https://github.com/kubijo/garmin-toolkit/releases/tag/v0.1.3) on ARM64. Populated synthetic
profiles, routes, Courses, and original downloads survived upgrade, restart, HAOS reboot, and cold restore. The checks
also verified app-data removal, empty reinstall, backup retention, and recovery followed by another upgrade. The schema
remained unchanged; future schema migrations and physical USB hardware require separate qualification.

## Withdrawal acceptance

The AMD64 [private-Store check](../../infra/integration/hass/README.md#withdrawal-and-replacement) used signed v0.1.2
and the [CI image](https://github.com/kubijo/garmin-toolkit/actions/runs/39745795968) under private version `0.1.3`,
with unchanged OCI bytes, HAOS 18.3, Supervisor 2026.10.3, and Core 2026.10.2. Store withdrawal preserved the installed
app, populated data, and downloads; replacement passed restart, reboot, and cold restore. Matching retries preserved
refs; stale promotions failed. At `1024×873`, app-owned header/navigation pixels matched browser captures across these
operations. Cleanup passed; public GitHub/GHCR refs were outside this check.

## Recovery checks

The [hosted recovery drill](https://github.com/kubijo/garmin-toolkit/actions/runs/40087044019) exercised publication
coordination, legacy history checkpoints, lost push replies, rollback, competing updates, withdrawal, and replacement.
Public refs were preserved and fixture cleanup passed. Its tiny OCI images, JSON assets, and injected qualification
failure did not exercise Supervisor installation failure, signed release recovery, or service outages.

The [signed installation check](../../infra/integration/hass/README.md#signed-installation-recovery) passed on AMD64
with HAOS 18.3, Supervisor 2026.10.3, and Core 2026.10.2, using signed
[v0.1.1](https://github.com/kubijo/garmin-toolkit/releases/tag/v0.1.1) →
[v0.1.2](https://github.com/kubijo/garmin-toolkit/releases/tag/v0.1.2). It exercised a Supervisor update failure,
metadata rollback, and retry with a private Git Store; public GitHub/GHCR refs remained unchanged.

## Dependency sources

Each release's signed inventory binds sources and notices to its shipped image, including inherited GVfs dependencies.
See the [source report contract](../../infra/release/README.md#dependency-source-bundles). SBOM `NOASSERTION` alone does
not establish redistribution coverage.

The reviewed ICU 78.3 source SHA-256 is `3a2e7a47604ba702f345878308e6fefeca612ee895cf4a5f222e7955fabfe0c0`. Its
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
