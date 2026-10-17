# HASS release candidates

These commands assemble candidates and verify contents, registry identities, and provenance. Publication and HAOS
qualification follow the
[publication plan](../../docs/plans/device-expansion-and-publication.md#hass-experimental-release). Outputs are never
overwritten; failures remove partial output.

## Generated Home Assistant repository

`packages.SYSTEM.garmin-hass-app-repository` generates an AMD64 store tree from canonical
[app metadata](../../apps/garmin-hass/addon/README.md) and shared brand assets. Its
[prescription](hass-app-repository.dirspec.toml) requires `repository.yaml` and app metadata, documentation, icon, and
logo. CLI generation accepts an explicit architecture list; advertise ARM64 only after runtime acceptance.

```sh
nix build .#garmin-hass-app-repository \
    --no-link --print-out-paths --max-jobs 1 --cores 2 --no-update-lock-file

just release::repository \
    --source . \
    --branding /nix/store/BRANDING-garmin-toolkit-brand-assets \
    --architectures amd64 \
    --output .tmp/hass-app-repository
```

Replace store-path placeholders with actual outputs. Generation leaves source metadata unchanged. Test the tree with the
[repository-install HAOS check](../integration/hass/README.md#repository-install-acceptance) before publishing
`hass-apps`.

## Native image inventories and build receipts

`packages.SYSTEM.garmin-hass-app-image` uses pinned [nix-oci](https://github.com/systemstart/nix-oci) to build an OCI
archive with source, revision, version, and license annotations. Registry preparation preserves manifest/config/layer
bytes.

`packages.SYSTEM.garmin-hass-app-sbom` produces SPDX 2.3 JSON for the corresponding production image. Build it on the
matching Linux architecture with the worker limits in
[development safeguards](../../docs/development.md#execution-safeguards):

```sh
nix build .#garmin-hass-app-image .#garmin-hass-app-sbom \
    --no-link --print-out-paths --max-jobs 1 --cores 2 --no-update-lock-file
```

Pinned Syft scans the image. Independent checks bind regular-file hashes, Nix store paths, embedded native/WASM
cargo-auditable dependencies, and reviewed asset notices to the SPDX inventory. Missing or conflicting records fail.
Hardlinks must target earlier regular files in the same layer; whiteouts and conflicting entries are rejected.

Archives and decompressed layers have 4 GiB limits and a combined 100,000-member limit; JSON metadata is separately
bounded. Scanner scratch stays under the Nix build directory or repository `.tmp` and is removed afterward. The SPDX
document records the original archive hash and uses the source epoch. An inventory does not establish provenance or
redistribution compliance.

Record the resulting files from a clean source checkout on the same native Linux platform:

```sh
just release::record-build \
    --source . \
    --archive /nix/store/IMAGE-garmin-toolkit-hass.oci.tar \
    --sbom /nix/store/SBOM-garmin-hass-image.spdx.json \
    --derivation /nix/store/IMAGE-garmin-toolkit-hass.oci.tar.drv \
    --output .tmp/hass-native-build
```

Obtain the derivation with `nix eval --raw .#garmin-hass-app-image.drvPath`. Recording verifies its native
system/output, source/image versions, runtime configuration, and SBOM, then exports the archive, SPDX file, and
`native-build.json`. Producing GitHub jobs supply their actual `--workflow-run`; local builds omit it.

Receipts bind archive/SBOM hashes and config/manifest digests. A matching version tag is recorded; branch builds use
`source.tag: null`. CI exports `garmin-hass-native-ARCH` from independent AMD64/ARM64 builders with verified two-CPU,
six-GiB daemon limits. Unsigned artifacts establish neither release provenance nor runtime qualification.

## Registry identity

Prepare an OCI layout from one or two native export directories:

```sh
just release::prepare-registry \
    --source . \
    --builds .tmp/hass-native-amd64 \
    --output .tmp/hass-oci-candidate
```

Preparation verifies native exports, matching source/version, manifest annotations, and layer `diff_ids`, then combines
unchanged platform manifests into an annotated index. Output contains `oci/` and `registry.json`, checked by
[prescription](hass-registry.dirspec.toml) and schema. Archive unpacking restores neither builder ownership nor
permissions.

Push with `skopeo copy --all --preserve-digests`, then independently verify the staged digest:

```sh
just release::verify-registry \
    --source . \
    --builds .tmp/hass-native-amd64 \
    --reference ghcr.io/kubijo/garmin-toolkit-hass@sha256:EXPECTED_INDEX_SHA256 \
    --acceptance .tmp/haos-evidence/SESSION \
    --output .tmp/hass-registry-verified
```

References must pin the expected index in the canonical GHCR repository or a literal loopback test registry. Pulls use
verified TLS and anonymous access unless `--authfile` is supplied; plain HTTP is loopback-only. Bounded preflight
rejects external blob URLs. Full verification rejects altered identities, platforms, descriptors, annotations, and layer
hashes. Decompressed registry layers share a 4 GiB budget. Only `registry.json` is retained; download scratch is
removed.

Optional `--acceptance` evidence must report success for the same source, native receipt, archive, installed
manifest/config, version, and architecture. Its hashes and runtime digest have scope `native-image-content`, which does
not qualify installation of the combined index. Remote acceptance and signed provenance remain publication gates.

QA runs a real disposable OCI Distribution registry with synthetic AMD64/ARM64 images; this tests transport, not ARM64
runtime support. To repeat the same test against an existing CI export and retain its small receipt:

```sh
GARMIN_REGISTRY_BUILD="$PWD/.tmp/hass-native-amd64" \
    GARMIN_REGISTRY_ACCEPTANCE="$PWD/.tmp/haos-evidence/SESSION" \
    GARMIN_REGISTRY_EVIDENCE="$PWD/.tmp/registry-evidence.json" \
    python -m unittest discover -s infra/python -p test_hass_registry.py -v
```

Run in the pinned tooling environment with Skopeo, Syft, `registry`, and `dirspec`. Tests clean their loopback processes
and temporary image copies.

## Inputs

Use a clean detached `vVERSION` source checkout reachable from fetched `origin/main` or local `main`. Versions are
numeric `MAJOR.MINOR.PATCH`; prerelease/build suffixes need a later version contract. Workspace/app/image versions and
the receipt's source commit and lockfile hash must agree. Fetch the release tag and main history before assembly.

The [assembly schema](hass-release.schema.json) requires source/tag/lock identity, the combined index digest, and native
platform receipts. Each platform supplies its unchanged OCI archive and nonempty SPDX 2.3 inventory under the names in
[hass-files.toml](hass-files.toml). Extra entries, links, directories, and empty files are rejected.

```sh
just release::assemble \
    --source /path/to/clean-tag-checkout \
    --receipt .tmp/native-builds.json \
    --artifacts .tmp/native-build-artifacts \
    --output .tmp/hass-release-candidate
```

## Output and verification

Assembly copies platform bytes, reads notices from committed blobs, and creates the workspace source archive with
`git archive`. It writes canonical `release.json` and GNU-compatible `SHA256SUMS` with `status: candidate` and no
acceptance results. Verification checks the complete inventory, hashes, source identity, and image metadata before
exposing output.

For local verification, obtain the manifest hash independently of the downloaded directory:

```sh
just release::verify \
    --directory .tmp/hass-release-candidate \
    --manifest-sha256 EXPECTED_RELEASE_JSON_SHA256 \
    --version 0.1.0 \
    --commit EXPECTED_FULL_SOURCE_COMMIT
```

`SHA256SUMS` includes `release.json`; neither file hashes itself. After establishing provenance, run
`sha256sum --check SHA256SUMS` inside the candidate directory. Local verification does not establish build provenance,
SBOM completeness, registry availability, or HAOS/USB acceptance.

## Signed provenance

`verify-provenance` uses `gh attestation verify` to authenticate `SHA256SUMS` first, then the manifest, local inventory,
payloads, and image digests. Supply an independently trusted version and commit:

```sh
just release::verify-provenance \
    --directory .tmp/hass-release-candidate \
    --bundles .tmp/hass-release-attestations \
    --version 0.1.0 \
    --commit EXPECTED_FULL_COMMIT
```

Keep bundles outside the release inventory: `checksums.sigstore.jsonl` authenticates the checksum file;
`artifacts.sigstore.jsonl` covers `release.json` and payloads; `sha256-DIGEST.sigstore.jsonl` covers each index/platform
manifest. This avoids checksum/signature cycles.

Policy requires `kubijo/garmin-toolkit/.github/workflows/hass-release.yml`, the exact tag and source/signer commit,
GitHub's OIDC issuer, SLSA provenance v1, and hosted runners. GitHub CLI needs registry/trust-root access. Actual
signing workflow issuance remains a [publication gate](../../docs/plans/device-expansion-and-publication.md); local
policy tests do not prove issuance.
