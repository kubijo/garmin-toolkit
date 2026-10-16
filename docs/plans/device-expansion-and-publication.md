# Device expansion and publication

Extend operation-specific device evidence, then publish only supported artifacts and claims.

## HASS experimental release

Deliver an installable Home Assistant app through GHCR and a GitHub Release, with verification and a documented first
workflow. This slice covers publication of existing functionality. Desktop and CLI distribution remain separate;
application authentication, credential custody, encryption, and Connect remain deferred.

### Source, versions, and builds

- Release from an explicit `vVERSION` tag reachable from `main`, with a clean source tree and locked dependencies.
  Require the workspace version, app metadata, image tag, and Home Assistant/OCI version labels to agree. Reject a
  branch checkout, mismatched revision, dirty input, missing platform, or reused version with different content.
- Build the production image once per selected architecture with the existing Nix derivations. Native AMD64 and ARM64
  builders produce `linux/amd64` and `linux/arm64` images; Home Assistant calls the latter `aarch64`. Verify the actual
  workers' hard limits of two CPUs and 6 GiB, and run heavy work sequentially per host. Reuse the shared Cargo target
  for local development; use the existing four-CPU disposable-VM exception only for acceptance runs.
- Promote the tested bytes without rebuilding. Record the source commit, source tag, lockfile hash, builder platform,
  derivation identity, workflow run, and runtime-test versions. Locked inputs and provenance do not establish bitwise
  reproducibility; make that claim only after an independent rebuild matches the published digests.

### Published artifacts and verification

- Publish `ghcr.io/kubijo/garmin-toolkit-hass:VERSION` as a multi-architecture index, retaining the digest of that index
  and of each platform manifest. OCI source, revision, version, and license annotations must identify this release. Keep
  version tags fixed; use digest references for machine verification and avoid a dependency on `latest`.
- Attach `garmin-toolkit-hass-VERSION-amd64.tar.gz` and `garmin-toolkit-hass-VERSION-aarch64.tar.gz` to the GitHub
  Release for the architectures included in that release. These are the Nix-built Docker image archives, retained
  without a second `docker save` or other repackaging. Also attach `garmin-toolkit-VERSION-source.tar.gz` with
  `flake.lock` and build instructions, `bundle-hass.json`, and the release's license/copyright/third-party notice files.
- Publish a machine-readable SPDX SBOM per platform covering the shipped image, including the Rust dependencies,
  frontend/assets, and native Nix runtime closure. Verify its contents against that image rather than presenting the
  Cargo notice list as a complete image inventory. Include the corresponding license/source material required for the
  bundled components; audit the actual distribution closure. Name each SBOM
  `garmin-toolkit-hass-VERSION-ARCH.spdx.json`, using `amd64` or `aarch64` for `ARCH`.
- Publish schema-validated `release.json` with the source/build identity above, each downloadable payload's filename,
  byte size and SHA-256, each platform's OCI manifest/config digests, the index digest, SBOM references, and acceptance
  results bound to those digests. Its file inventory excludes itself, `SHA256SUMS`, and attestation envelopes to avoid
  cycles. Define deterministic serialization and reject duplicate/missing platforms or files.
- Publish `SHA256SUMS` for every release payload and descriptive file, including `release.json`, source, SBOMs, and
  notices. Exclude the checksum file itself and its attestation envelopes to avoid self-reference. Derive the list from
  the validated artifact inventory, then independently download and hash the public release assets.
- Keep archive SHA-256, OCI index digest, platform manifest digest, image-config digest, and Nix store identity distinct
  in the schema and user instructions. Hashes identify different objects; an image archive's hash is not a pull digest.
  The [OCI descriptor contract](https://specs.opencontainers.org/image-spec/descriptor/) defines registry content
  digests.
- Generate GitHub OIDC build-provenance attestations for the archives and published image digests, and attest the
  aggregate `release.json` and `SHA256SUMS`. Publish downloadable attestation bundles as well as registry attestations.
  Verification must enforce `kubijo/garmin-toolkit`, the release workflow identity, source tag, source commit, and
  expected subject digest. A hash alone checks content against an expected value; provenance also identifies its
  producing workflow. Follow [GitHub's attestation interface](https://cli.github.com/manual/gh_attestation_verify).
- Document and exercise both `sha256sum --check SHA256SUMS` and `gh attestation verify` for files and image digest
  references. Verify checksum provenance before trusting the checksum list. Acceptance must reject a modified file,
  substituted image/platform, wrong source revision, unexpected signer, missing attestation, or incomplete inventory.
- Keep artifact prescriptions, release schemas, templates, and verification tooling in filesystem-backed files enrolled
  in QA. Extend `dirspec` checks to the assembled release directory so missing, empty, or unexpected deliverables fail
  immediately. Python tooling uses Tyro and the maintained formatter/checkers.

### Installable repository and publication order

- Generate a release-only `hass-apps` branch in this GitHub repository from the tagged source. It contains root
  `repository.yaml` and `garmin_toolkit/` with `config.yaml`, README, DOCS, CHANGELOG, icon, and logo. Keep the existing
  slug and generate from canonical source files and brand assets; do not maintain an independent handwritten copy. The
  proposed store URL is `https://github.com/kubijo/garmin-toolkit#hass-apps`. Supervisor's
  [repository parser](https://github.com/home-assistant/supervisor/blob/main/supervisor/validate.py) accepts a branch
  suffix; verify the complete URL in the supported HAOS version before advertising it. The root manifest follows
  [Home Assistant's repository contract](https://developers.home-assistant.io/docs/apps/repository/).
- Retain `stage: experimental`, ingress, and cold `/data` backup. Keep production control routes disabled. Request only
  the USB/udev access supported by the packaged adapter and acceptance evidence. App `version` selects the image tag;
  `image` names the generic multi-architecture repository, as specified by
  [Home Assistant's app configuration](https://developers.home-assistant.io/docs/apps/configuration/).
- Add a maintainer-triggered workflow dispatched at the release tag. PRs validate and build without publication
  credentials. Pin release actions, grant write/OIDC permissions only to the jobs that need them, serialize promotion
  across app releases, and hard-fail on incomplete architecture output or an existing version/digest conflict.
- Stage uniquely named candidate image references and repository metadata first. Verify the candidate index and each
  platform through registry pulls, then exercise the generated repository in disposable HAOS. Extend the maintained
  harness to accept candidate registry/repository references; retain the existing production app and semantic workflow.
  This must cover store discovery and installation, rather than installing only a copied local app directory.
- After acceptance, assign the final version reference to the tested index, verify final registry digests, and upload
  the complete assets and attestations to a draft GitHub Release. Re-download and verify them, publish the release, then
  advance `hass-apps` to expose that accepted version. Test the final public store URL and anonymous image pulls. GHCR
  visibility must allow installation without the developer's credentials, as supported by
  [GitHub's registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).
  If the final public install probe fails, restore the previous advertised metadata and mark the release failed while
  retaining its artifacts for diagnosis.
- Retries must verify and reuse identical staged artifacts or stop; never replace a published version with new bytes. A
  failed build, verification, or acceptance run before promotion must leave the advertised app version unchanged.
  Preserve previous images, source, hashes, and attestations for backup restoration. Withdraw a bad version through
  repository metadata and release notes while retaining its evidence; do not silently repoint its tag or promise
  database downgrade.

### Acceptance and user documentation

- Extend the [populated HAOS workflow](../../infra/integration/hass/README.md#populated-haos-workflow) to install via
  the generated app repository and registry. Assert the selected platform and exact digest, then cover authenticated
  ingress, profile creation, GPX import, FIT Course generation, byte-verified downloads, app/HAOS restart, and cold
  Supervisor backup/restore against an independent storage oracle.
- Cover upgrade from the previous advertised version with populated state and backup before update. For the first
  release, use an explicitly identified unpublished fixture baseline and record that no previous public version exists.
  Demonstrate recovery by reinstalling the compatible previous image and restoring its backup; do not equate this with
  opening a newer database in an older executable. Verify uninstall/reinstall with disposable state and document which
  data survives and which is removed.
- Qualify every advertised architecture with its own HAOS install/ingress/lifecycle evidence. An ARM64 build or an AMD64
  VM does not qualify ARM64 runtime support. If ARM64 acceptance remains pending, exclude it from the store's supported
  architecture list and record the limit; its builder work must not block an AMD64-only experimental release.
- Keep physical USB claims separate from software acceptance. The [HASS hardware plan](hass-watch-vertical-slice.md)
  owns Raspberry Pi permissions, attachment/disconnect, pairing/reassociation, and interrupted Course transfer/recovery.
  Publish only tested device/firmware/host/operation combinations; mark other combinations unknown or unsupported.
- Write a clean-install quickstart, the first useful GPX-to-FIT workflow, device attachment expectations,
  persistent-data location, update/backup/restore/removal instructions, verification commands, troubleshooting, and
  supported versions. Explain trusted local operation and the existing plaintext storage/snapshot limits without making
  the deferred security stage a release prerequisite. Include where to report failures and how to provide redacted
  diagnostics.
- Capture production ingress screens in dark mode with synthetic data using the established app control/screenshot
  mechanism, and document browser-only gaps. Bind acceptance results and image evidence to the release commit and
  artifact digests. Keep durable results with the release; remove VM disks, credentials, browser profiles, listeners,
  and owned processes after both success and failure. Preserve the execution safeguards in
  [development](../development.md).

### Implementation slices and completion

1. Extend the [local candidate contract](../../infra/release/README.md) with producing CI receipts, actual registry
   digest verification, and signed provenance verification. Qualify the native image/SBOM derivations with real capped
   AMD64 and ARM64 builds and confirm emitted CI receipts. Local assembly/content checks and synthetic scanner tests do
   not qualify publication.
2. Produce the app repository layout and user documentation from canonical source; extend the existing HAOS harness for
   registry/repository installation, platform/digest checks, upgrade, and cleanup.
3. Add the capped native build, provenance, staging, promotion, and release workflow. Validate the complete candidate
   artifacts and attestations before making the version discoverable.
4. Run public download/verification and clean-install acceptance, then record supported platforms/operations and publish
   the experimental release. Track the physical hardware work with its existing owner.

Completion means a new user can add the documented repository, install a qualified platform image without registry
credentials, complete the GPX-to-FIT workflow, update and recover data as documented, and independently verify the
downloaded bytes and their build identity. Release failures do not advertise incomplete versions. This plan section
remains open until that path works with the final published artifacts.

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
