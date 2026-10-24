# HASS release candidates

Build, verify, and publish candidates under the
[publication plan](../../docs/plans/device-expansion-and-publication.md). Candidate commands reject existing output
directories.

## GitHub publication

Before tagging, dispatch [HASS experimental release](../../.github/workflows/hass-release.yml) from a trusted branch
with **Publish disabled**:

```sh
gh workflow run hass-release.yml --ref BRANCH -f publish=false
```

Branch preflight builds AMD64 and ARM64 images and sources, stages one combined index, tests HAOS lifecycle and
repository installation on each architecture, and signs files and OCI digests. A fresh read-only job verifies the
artifact inventory, branch/commit signatures, and acceptance archive. Invalid payloads, revisions, signers, and platform
subjects must fail. It creates no GitHub release and does not advance `hass-apps`. Branch evidence cannot qualify a
tagged release.

Dispatch [HASS experimental release](../../.github/workflows/hass-release.yml) from a clean `vVERSION` tag reachable
from `main`, matching workspace/app versions:

- **Publish disabled:** build both native images and source bundles, stage unique references, run HAOS and provenance
  rejection checks, sign artifacts, and independently verify draft downloads.
- **Publish enabled:** authenticate and promote the original tested bytes, publish the release, advance `hass-apps` with
  a branch lease, and verify public installation on both architectures. Either failure restores the previous store
  metadata.

CI is read-only. Publication is manual, serialized, and restricted to this repository. GHCR must allow anonymous pulls.
Conflicts and incomplete drafts fail without replacing published bytes. Cleanup removes candidate branches and
credentials.

For [focused ARM64 delivery acceptance](../integration/hass/README.md#staged-arm64-delivery), set `arm64_run` to a
successful CI run ID with Publish and Recovery disabled. This reuses its native image without builds or promotion.

The `hass-publication` branch records intent and qualification in `publication.json` with readable commit subjects.
Readers also accept legacy state JSON in empty-tree commit messages. State advances atomically with `hass-apps`;
recovery advances it even when Store metadata is unchanged to fence delayed pushes. Keep the state branch and follow
[publication recovery](#publication-recovery) for retries.

Cancellation reclaims owned acceptance and VM scopes, including detached descendants. Scratch is removed only after both
scopes are empty; cleanup failure retains it and reports its path. Evidence remains outside disposable scratch.

[Python phases](../python/hass_ci.py) own orchestration; [runner setup](../ci/runner.sh) owns host preparation and
cleanup. Lifecycle checks exercise upgrades, backup recovery, and reinstall with synthetic profile, route, and Course
data. Published baselines are authenticated against their signed inventory and source tag. Metadata-only fixtures must
report `migration_compatibility: false`; they do not establish older-binary or schema migration compatibility.

Native inputs and sources use `amd64/` and `aarch64/` directories. The signed acceptance archive contains both
directories, each with runtime versions, reports, and dark captures, plus one shared staging journal. Verification
requires both platform manifests and guest reports. Existing signed AMD64 releases retain their flat evidence layout and
AMD64-only Store metadata when retried.

Assembly shares immutable payload files through hardlinks within scratch, avoiding extra copies of both large source
bundles. Cross-filesystem inputs fall back to copying; content and provenance checks still verify every platform.

## Generated Home Assistant repository

`garmin-hass-app-repository` builds an AMD64/ARM64 Store tree from canonical metadata and branding, checked by
[dirspec](hass-app-repository.dirspec.toml). Publication selects the signed inventory's architectures and qualifies
every platform before recording success.

```sh
nix build .#garmin-hass-app-repository \
    --no-link --print-out-paths --max-jobs 1 --cores 2 --no-update-lock-file
```

For manual generation, use `just release::repository --help`. Run the
[repository-install check](../integration/hass/README.md#repository-install-acceptance) before publication.

## Native image inventories and build receipts

Pinned [nix-oci](https://github.com/systemstart/nix-oci) produces the annotated OCI archive; Syft produces SPDX 2.3.
Build on the matching Linux architecture, following the
[execution safeguards](../../docs/development.md#execution-safeguards):

```sh
nix build .#garmin-hass-app-image .#garmin-hass-app-sbom \
    --no-link --print-out-paths --max-jobs 1 --cores 2 --no-update-lock-file
```

Inventory checks bind file hashes, Nix paths, cargo-auditable native/WASM dependencies, and asset notices. Whiteouts,
conflicting entries, and hardlinks without earlier regular targets are rejected. Archives and decoded layers have 4 GiB
/ 100,000-member limits. Scratch is removed afterward.

Record outputs from a clean checkout on the same native host:

```sh
just release::record-build \
    --source . \
    --archive /nix/store/IMAGE-garmin-toolkit-hass.oci.tar \
    --sbom /nix/store/SBOM-garmin-hass-image.spdx.json \
    --derivation /nix/store/IMAGE-garmin-toolkit-hass.oci.tar.drv \
    --output .tmp/hass-native-build
```

Get the derivation with `nix eval --raw .#garmin-hass-app-image.drvPath`. Recording verifies system/output, versions,
runtime configuration, and SBOM, then exports the archive, SPDX file, and `native-build.json`. Receipts bind source,
lockfile, archive/SBOM hashes, and config/manifest digests. Branch builds use `source.tag: null`.

CI exports `garmin-hass-native-ARCH` from native AMD64/ARM64 builders. Producing jobs supply their actual
`--workflow-run`; local builds omit it.

## Dependency source bundles

Dispatch read-only CI with **sources enabled** to audit both architectures before tagging. For local export, evaluate
`legacyPackages.SYSTEM.hass-source-metadata` into a JSON file, then run:

```sh
just release::bundle-sources \
    --archive /nix/store/IMAGE-garmin-toolkit-hass.oci.tar \
    --metadata .tmp/dependency-metadata.json \
    --output .tmp/hass-native-sources --realise
```

The [export](hass-sources.dirspec.toml) contains an image-bound license/source report and deterministic archive of
transitive sources, patches, fixed-output inputs, vendored Cargo sources, and build recipes. Its dependency graph binds
each recipe to required inputs; verification rejects missing upstream sources and recipe-only runtime coverage. Entries
retain their Nix store basenames under `sources/`. The workspace archive and `flake.lock` provide build instructions;
build tools may still need downloading.

`--realise` fetches missing sources through their recorded fixed-output derivations, with Nix verifying the pinned
hashes. Other missing source paths require a substitute; compiled package outputs stay outside the request. Agent/CI
callers must cap actual Nix workers. The compressed stream splits into at most 16 checksummed parts of up to 1.5 GiB
each. The first retains the `.tar.gz` name; subsequent parts append `.002`, `.003`, and so on. Verification streams them
in order without joining them on disk. The whole archive is limited to 32 GiB decoded and 2,000,000 members. A
metadata-only preflight reports exact tar size and entry count and rejects oversized exports before compression. The
writer and verifier enforce the same limits. OCI image limits remain 4 GiB / 100,000 members. Unknown/nonfree licenses,
missing recipes, or altered archives block publication. Audit the downloaded sources and notices against the shipped
closure.

## Registry identity

Prepare an index from one or two native exports:

```sh
just release::prepare-registry \
    --source . \
    --builds .tmp/hass-native-amd64 \
    --output .tmp/hass-oci-candidate
```

Preparation verifies source/version, annotations, and layer `diff_ids`, preserving platform manifest/config/layer bytes.
Output is `oci/` and `registry.json`, checked by [dirspec](hass-registry.dirspec.toml) and schema.

Push with `skopeo copy --all --preserve-digests`, then verify independently:

```sh
just release::verify-registry \
    --source . \
    --builds .tmp/hass-native-amd64 \
    --reference ghcr.io/kubijo/garmin-toolkit-hass@sha256:EXPECTED_INDEX_SHA256 \
    --output .tmp/hass-registry-verified
```

References pin the canonical GHCR repository or literal loopback registry. Pulls are anonymous unless `--authfile` is
supplied; HTTP is loopback-only. Verification rejects external blob URLs, altered identities, platforms, descriptors,
annotations, and layer hashes. Decoded layers share a 4 GiB budget; only `registry.json` survives cleanup.

Optional `--acceptance` binds successful evidence to source, native receipt, archive, installed manifest/config, archive
version, and architecture. Its `native-image-content` scope does not qualify combined-index installation.

QA exercises a disposable Distribution registry with synthetic AMD64/ARM64 images. To test a retained CI export:

```sh
GARMIN_REGISTRY_BUILD="$PWD/.tmp/hass-native-amd64" \
    GARMIN_REGISTRY_ACCEPTANCE="$PWD/.tmp/haos-evidence/SESSION" \
    GARMIN_REGISTRY_EVIDENCE="$PWD/.tmp/registry-evidence.json" \
    python -m unittest discover -s infra/python -p test_hass_registry.py -v
```

Use the pinned tooling environment with Skopeo, Syft, `registry`, and `dirspec`.

## Inputs

Assembly requires a clean detached `vVERSION` checkout reachable from fetched `origin/main` or local `main`. Numeric
`MAJOR.MINOR.PATCH`, workspace/app/image versions, source commit, and lockfile must agree.

The [schema](hass-release.schema.json) and [inventory](hass-files.toml) require source/tag/lock identity, index digest,
native receipts, and each platform's OCI archive, SPDX inventory, and dependency source archive/report. Extra entries,
links, directories, and empty files fail.

```sh
just release::assemble \
    --source /path/to/clean-tag-checkout \
    --receipt .tmp/native-builds.json \
    --artifacts .tmp/native-build-artifacts \
    --output .tmp/hass-release-candidate
```

## Output and verification

Assembly preserves platform bytes, reads committed notices, and uses `git archive` for workspace sources. It writes
canonical `release.json` and `SHA256SUMS` with candidate status and no acceptance claims.

Verify using an independently obtained manifest hash:

```sh
just release::verify \
    --directory .tmp/hass-release-candidate \
    --manifest-sha256 EXPECTED_RELEASE_JSON_SHA256 \
    --version VERSION \
    --commit EXPECTED_FULL_SOURCE_COMMIT
```

`SHA256SUMS` includes `release.json`; neither hashes itself. Authenticate provenance before running
`sha256sum --check SHA256SUMS`. Content checks do not establish provenance, redistribution compliance, or HAOS/USB
support.

## Signed provenance

Verification authenticates `SHA256SUMS` first, then the manifest, payload inventory, and image digests. Supply an
independently trusted version and commit:

```sh
just release::verify-provenance \
    --directory .tmp/hass-release-candidate \
    --bundles .tmp/hass-release-attestations \
    --version VERSION \
    --commit EXPECTED_FULL_COMMIT
```

Keep bundles outside the payload inventory: `checksums.sigstore.jsonl` authenticates `SHA256SUMS`;
`artifacts.sigstore.jsonl` covers the manifest and payloads; `sha256-DIGEST.sigstore.jsonl` covers index/platform
manifests. `acceptance.sigstore.jsonl` authenticates `haos-acceptance.tar.gz` before bounded extraction of reports and
dark captures.

`gh attestation verify` enforces this repository's `hass-release.yml`, exact tag and source/signer commit, GitHub OIDC,
SLSA v1, and hosted runners. It needs registry/trust-root access. Every candidate must pass these checks.

## Publication recovery

Keep the workflow run, tag, source commit, release asset inventory, image digest, and both branch heads with the
incident. The workflow's publication/promotion evidence contains its staging and promotion journals. A timeout can
follow a successful remote write; inspect the durable `hass-publication` state before deciding whether an operation
failed.

- **Version conflict:** retain the existing tag, release assets, and image bytes. A different image under the same
  version is fatal. Prepare a greater version from the corrected source and run its full release qualification.
- **Incomplete draft:** promotion rejects missing or unexpected assets before changing the registry or store. Inspect
  the draft and its original run; do not fill gaps with bytes from another build or use `--clobber`. If the original
  signed candidate cannot be recovered intact, explicitly delete only the unpublished draft after checking `isDraft`
  with `gh release view vVERSION --json isDraft,assets`. Retain the source tag and run evidence, then dispatch that tag
  with **Publish disabled** to assemble and independently verify a fresh draft. A public release requires withdrawal or
  a new version, rather than this draft cleanup.
- **Failed installation:** the promotion workflow restores the previous store head and marks the attempt aborted. Its
  release and versioned image may already be public; rollback preserves those artifacts. Fix the installation problem,
  then dispatch the same tag with **Publish enabled**. The retry authenticates the original signed downloads and repeats
  installation qualification without uploading replacement assets or rebuilding the image.
- **Interrupted attempt:** a fresh promotion reconciles unfinished durable state before advertising metadata. Failed
  retries preserve a previously qualified store head. Recovery also fences a delayed push whose caller already exited.
- **Changed store or state branch:** retain the new heads and stop. Recovery refuses unrelated metadata rather than
  force-pushing over it. Establish which publication owns the change before retrying; keep both state histories.

The disposable-Git tests in [publication](../python/test_hass_publish.py),
[CI orchestration](../python/test_hass_ci.py), and [withdrawal](../python/test_hass_withdraw.py) cover leases, atomic
updates, interrupted pushes, failed acceptance, retries, and stale recovery. They do not qualify GitHub/GHCR outages or
Supervisor behavior.

### Publication history checkpoint

Home Assistant installation URLs require `hass-apps`. Before checkpointing `hass-publication`, deploy a compatible
publisher and finish active or queued release runs. The first state-file write makes legacy tagged publishers
incompatible; finish releases using them before writing the new format. Subsequent publication requires compatible tags.

Inspect the current qualified state and read both exact heads as in [withdrawal](#release-withdrawal), then run:

```sh
just release::checkpoint-publication \
    --store-commit EXACT_HASS_APPS_COMMIT \
    --state-commit EXACT_HASS_PUBLICATION_COMMIT \
    --output .tmp/publication-checkpoint
```

The command rejects unfinished or unqualified state and active or queued publication workflows. It atomically replaces
the state branch with one root commit and appends a Store commit with identical files. Both refs must move to enforce
both exact leases on the server. Source history and release tags remain untouched.

Keep `checkpoint.bundle` and `checkpoint.json` together: they preserve the old history and exact replacement objects. If
a push response is lost, rerun the same command with the same output directory. It retries those objects or recognizes
the completed update; competing changes stop the operation. This guarded checkpoint is the sole history-rewrite path.

### Recovery drill

The release workflow's **Recovery** mode tests production guards against synthetic GitHub/GHCR fixtures: image
conflicts, incomplete drafts, interrupted preparation, lost atomic-push replies, failed qualification, retries, and
competing updates. Dispatch after pushing the tested revision:

```sh
gh workflow run hass-release.yml --repo kubijo/garmin-toolkit --ref BRANCH -f recovery=true -f publish=false
```

Run/attempt IDs scope two `hass-recovery-RUN-ATTEMPT-{apps,state}` branches, a `hass-recovery-RUN-ATTEMPT` tag and draft
release, and the `ghcr.io/kubijo/garmin-toolkit-recovery-RUN-ATTEMPT` package. The drill requires the exact checkout on
a GitHub-hosted Linux runner with its repository token and shares publication concurrency. Pushes outside owned refs are
rejected; public store/release refs must stay unchanged.

Cleanup checks recorded commits, exact leases, asset bytes, and package versions, retaining unexpected changes.
Credentials are removed even if Nix cannot start. Both `result.json` and `cleanup.json` must pass in the
`hass-recovery-evidence` artifact; cleanup failure fails the job.

The [signed installation check](../integration/hass/README.md#signed-installation-recovery) uses authenticated public
images with a private store to exercise a Supervisor update failure, rollback, and retry. Validation scope is recorded
in [recovery evidence](../../docs/research/publication-readiness.md#recovery-checks).

## Release withdrawal

Withdraw a qualified release when it must stop being offered for new installations. This removes `garmin_toolkit/` from
`hass-apps` and records `withdrawn` state atomically, retaining repository metadata and both Git histories. It does not
contact Home Assistant instances, stop installed apps, restore their data, or downgrade them. Store clients must refresh
their cached repository metadata. The
[disposable withdrawal check](../integration/hass/README.md#withdrawal-and-replacement) covers Supervisor refresh,
installed data, and replacement upgrades; public GitHub/GHCR recovery remains separate.

First ensure no publication run is active or queued. Record the current release version and read both exact heads:

```sh
gh api repos/kubijo/garmin-toolkit/git/ref/heads/hass-apps --jq .object.sha
gh api repos/kubijo/garmin-toolkit/git/ref/heads/hass-publication --jq .object.sha
```

Review those commits and the durable state's `qualified` status. After explicit authorization for the affected release:

```sh
just release::withdraw \
    --version VERSION \
    --store-commit EXACT_HASS_APPS_COMMIT \
    --state-commit EXACT_HASS_PUBLICATION_COMMIT \
    --reason 'Describe the defect and recovery guidance' \
    --output .tmp/withdrawal-VERSION
```

The command requires the currently qualified app identity/version and both unchanged heads. A concurrent update rejects
the entire push. Keep `withdrawal.json`; if the response is lost, read the remote heads and rerun the exact request with
a fresh output directory. An already completed matching withdrawal succeeds without another remote write. Changed heads,
a different reason, or unfinished publication state require review.

Mark the GitHub release title as withdrawn and prepend the reason, affected users, backup/recovery guidance, and any
replacement version to its existing notes. Keep the original notes and prerelease status. Use
[`gh release edit --notes-file`](https://cli.github.com/manual/gh_release_edit) with a reviewed file. Retain the release
tag, checksums, attestations, image tags/digests, source bundles, and acceptance evidence. Do not reuse the withdrawn
version or rewind `hass-publication`. Stale promotion and rollback attempts cannot replace the withdrawal metadata.

Publish a greater, fully qualified version to restore the app to the store. Users already affected need explicit
recovery guidance and, where applicable, their own backup restore; withdrawing metadata performs neither action.
