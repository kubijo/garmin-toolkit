# Publication readiness

## Dependency sources

The [release inventory](../../infra/release/hass-files.toml) includes workspace source and Cargo/asset notices, but
omits runtime Nix dependency sources. Supplemental [SBOM records](../../infra/python/hass_image_sbom.py) use
`NOASSERTION` for licensing. A closure-wide license/source audit remains required.

The [HASS package](../../apps/garmin-hass/flake.nix) inherits standalone GVfs dependencies despite selecting raw MTP.
Those dependencies are included in the audit; the build-only OCI writer is not shipped.

Choose a source delivery approach before fixing the publication inventory:

- **Source bundles (recommended):** ship exact dependency sources, notices, and build instructions with checksums and
  attestations. Larger downloads; independent of upstream availability.
- **Maintained source mirror:** publish a digest-bound index and retain sources/notices at project-controlled URLs. Less
  release duplication; ongoing hosting and retention.

Verify the selected delivery path and downloaded contents before clearing the
[publication gate](../plans/device-expansion-and-publication.md#provenance-and-redistribution).

## Distribution choices

HASS targets AMD64 experimental releases through GHCR and GitHub Releases. Require anonymous pulls and store acceptance;
advertise ARM64 only after its runtime checks pass.

Desktop choices are the existing AppImage or native packages with additional installation/update tests. Select supported
hosts and signing identity before publication. Mobile remains separate.
