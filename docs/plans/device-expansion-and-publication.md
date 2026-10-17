# Device expansion and publication

Extend operation-specific device evidence, then publish only supported artifacts and claims.

## HASS experimental release

Publish the existing app through GHCR and a GitHub Release using the [candidate contract](../../infra/release/README.md)
and [repository-install check](../../infra/integration/hass/README.md#repository-install-acceptance). Desktop/CLI
distribution, application authentication, credential custody, encryption, and Connect remain separate work.

### Provenance and redistribution

- Generate GitHub OIDC attestations for archives, published image digests, `release.json`, and `SHA256SUMS`. Publish
  downloadable bundles and registry attestations. Verification must enforce `kubijo/garmin-toolkit`, the release
  workflow, source tag/commit, and subject digest; see
  [GitHub's interface](https://cli.github.com/manual/gh_attestation_verify).
- Exercise [provenance verification](../../infra/release/README.md#signed-provenance) with real workflow-issued bundles
  and registry attestations, including altered payloads/platforms, wrong revisions/signers, and missing inventory.
- Complete the shipped closure's license/source audit and select a
  [source delivery approach](../research/publication-readiness.md#dependency-sources); an SBOM alone does not satisfy
  it.

### Lifecycle acceptance

- Run install/ingress/lifecycle acceptance against the staged, release-tagged registry image on every advertised
  architecture. Exclude ARM64 from store metadata until its runtime acceptance passes; pending ARM64 work must not block
  an AMD64-only release.
- Keep physical-device claims within the [HASS hardware plan](hass-watch-vertical-slice.md) and publish only tested
  device/firmware/host/operation combinations.

### Publication workflow

- Add a maintainer-triggered workflow for a clean `vVERSION` tag reachable from `main`, reusing native build jobs and
  [execution safeguards](../development.md#execution-safeguards). Pin actions, restrict write/OIDC permissions,
  serialize promotion, and reject incomplete platforms or version/digest conflicts. PR jobs receive no publication
  credentials.
- Stage uniquely named image references and generated repository metadata. Verify registry pulls and run the
  repository-install check against the remote candidate, retaining experimental status and the production app.
- Promote the tested index without rebuilding or repackaging. Verify final registry digests, upload the complete
  candidate and attestations to a draft GitHub Release, then independently download and verify them.
- Publish the release before advancing the release-only `hass-apps` branch. Test
  `https://github.com/kubijo/garmin-toolkit#hass-apps` and anonymous GHCR pulls. If final installation fails, restore
  the previous advertised metadata and retain failure evidence.
- Retries must reuse identical bytes or stop. Keep version tags immutable and preserve previous artifacts for recovery.
  Failures before promotion leave the advertised version unchanged; withdraw bad versions through metadata/release
  notes.

### User documentation

- Update the [quickstart](../../apps/garmin-hass/addon/DOCS.md) with the public URL, qualified Home Assistant versions,
  architectures, hardware limits, update/recovery behavior, and checksum/provenance commands.
- Attach digest-bound acceptance results, runtime versions, and dark app captures to the release.

## Hardware and other distribution

1. Extend [current map evidence](../research/usb-sync.md#map-maintenance-evidence) to full fēnix 8 Solar and Edge 1050
   conformance. Validate Venu 3S, then Edge 850, following the
   [hardware matrix](../research/device-capabilities.md#validation-hardware).
2. Record identity, metadata, I/O, interruption, preservation, normalization, rendering, export, and recovery per
   device/firmware/host tuple. Mark unsupported cells. Compare CLI, desktop, and HASS inspection and refresh on the same
   physical device, including separate volumes, missing attributes, and read-only media. Check disconnects and stale
   attachment results. Complete consented Venu 3S explorer acceptance; do not hide host/GVFS defects.
3. Investigate battery, charging, and transport health before exposing them. Classify portability, reliability, and
   sensitivity; retain no field without adapter and hardware evidence.
4. Run the local Connect IQ probe after HASS HTTPS exists.
5. Choose desktop release channels, signing and attestations; HASS publication is specified above. Reuse applicable
   [CLI distribution evidence](distribution.md). Track the host AMD-to-NVIDIA presentation failure, also reproduced with
   independent `vkcube`, separately from packaging acceptance. Extend the
   [NVIDIA and software-Mesa smoke evidence](../architecture/build-system.md#desktop-packaging) to other non-Nix hosts
   and aarch64. Keep driver integration independent of the build host's GPU and driver version. Validate user-namespace
   availability, file dialogs, production USB/GVfs access, and launcher metadata on supported hosts; a successful bundle
   build alone is not portability evidence. On additional supported hosts, verify Developer tools remains interactive
   with the main window fully covered, including cursor changes, text selection, scrolling, clicks, folder opening, and
   closing/reopening the tools window.
6. Audit configuration, fixtures, identities, URLs, images, notices, and provenance.
7. Publish the support matrix and adaptation guide. Mobile requires a separate decision.
