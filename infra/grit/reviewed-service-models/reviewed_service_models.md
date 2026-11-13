---
level: error
---

# Require review for public service models

Public service-boundary models must be explicitly reviewed and added to the approved model set.

The map workflow models use owned, Postcard-compatible values and fixed-width counts. Commands carry opaque request,
revision, and approval identities; host captures, transport handles, payloads, and raw progress events stay server-side.
`DeviceFitImportPlan` carries opaque review/job IDs and the existing owned FIT preview; device bytes stay server-side.
`DeviceFitImportStatus` carries only a fixed lifecycle state; the host retains the running job and stored acquisition.
FIT preview content distinguishes activities from read-only courses. Course previews carry owned names, canonical
geometry, an optional sport, and a fixed-width cue count; they do not authorize activity imports.

Cloud `ConnectFitPreview` carries the existing normalized FIT preview, an activity ID, and a digest binding the profile,
activity ID, account and reviewed FIT bytes. `ConnectActivityResult` distinguishes a bounded interactive preview,
compact batch preparation (ID, digest and imported status), and list/import results. Interactive previews are capped at
8 MiB encoded before entering browser RPC; batch preparation never transfers recording samples. Cloud activity rows
include profile- and account-scoped imported status derived from successful stored observations. Cloud previews also
carry bounded user-authored activity names/descriptions and account-scoped imported status. Activity failures
distinguish one-file errors from session errors, so explicit page imports stop on access or rate limits without parsing
diagnostic text. Batch queues and native-size progress counts stay client-side. Credentials and raw archives stay
host-side; explicit import rechecks the saved key, account and digest before using the existing importer.

Route contracts contain owned metadata, canonical coordinates and sports, opaque operation/artifact IDs, and bounded
upload or geometry chunks. Sessions bind the profile and database epoch; requests carry no filesystem paths, parser
selection, or replacement geometry. Import and generation receipts identify immutable saved results. Downloads use the
existing single-use ticket contract after route ownership and stored-byte integrity checks. Candidate outlines contain
at most 64 normalized two-byte points; full geometry is fetched in bounded chunks after selection.

Course transfer contracts expose an owned Course version, manifest-declared device destination, opaque one-use approval,
and readback status. The host retains transport, payload, journal, and mutation lock. A profile authorizes its Course
bytes; choosing a device does not pair or reassign that device. Cleanup has a separate approval bound to proven partial
bytes and the exact retained path. `CourseTransferProgress` exposes only bounded byte counts and a finalizing flag for
the active transfer; raw device events and paths remain on the host.

```grit
language rust

or {
  struct_item(name=$name) as $item,
  enum_item(name=$name) as $item,
  type_item(name=$name) as $item
} where {
  $item <: contains visibility_modifier(),
  $name <: not or {
    `Action`,
    `RouteSummary`,
    `RouteSource`,
    `CourseVersion`,
    `CourseTarget`,
    `CourseTransferReview`,
    `CourseTransferPhase`,
    `CourseTransferProgress`,
    `CourseTransferStatus`,
    `CourseTransferPreparation`,
    `CourseCleanupReview`,
    `GpxCandidate`,
    `OutlinePoint`,
    `GpxRejected`,
    `GpxUploadPhase`,
    `GpxUpload`,
    `RouteSelection`,
    `RouteRequest`,
    `RouteReply`,
    `RouteFailureKind`,
    `RouteFailure`,
    `Phase`,
    `Choice`,
    `CatalogService`,
    `Component`,
    `Plan`,
    `StorageRequirement`,
    `Recovery`,
    `RecoveryReview`,
    `RecoveryFile`,
    `Progress`,
    `ProgressStatus`,
    `Outcome`,
    `State`,
    `Command`,
    `Request`,
    `FailureKind`,
    `Failure`,
    `DeviceSnapshot`,
    `SnapshotState`,
    `Directory`,
    `Operation`,
    `Selection`,
    `SnapshotPreview`,
    `SnapshotStatus`,
    `SnapshotSource`,
    `SnapshotOperation`,
    `SnapshotRequest`,
    `SnapshotReply`,
    `SnapshotFailureKind`,
    `SnapshotFailure`,
    `ControlCommand`,
    `ControlSession`,
    `ControlDispatch`,
    `CaptureInfo`,
    `Capture`,
    `Batch`,
    `DeviceCatalogSnapshot`,
    `DeviceCatalogStorage`,
    `DeviceCatalogEntry`,
    `DeviceCatalogEntryKind`,
    `DeviceBrowserTarget`,
    `DeviceBrowserRequest`,
    `DeviceBrowserUpload`,
    `DownloadTicket`,
    `DeviceFitPreview`,
    `DeviceFitPreviewContent`,
    `DeviceFitPreviewCourse`,
    `DeviceFitImportPlan`,
    `ConnectFitPreview`,
    `ConnectActivityResult`,
    `DeviceFitPreviewActivity`,
    `DeviceFitImportOutcome`,
    `DeviceFitImportStatus`,
    `DeviceCapability`,
    `DeviceDataType`,
    `DeploymentMode`,
    `InspectionState`,
    `TransferDirection`,
    `ProfileSnapshot`,
    `ProfileAvatarSnapshot`,
    `AvatarUpload`,
    `AvatarCrop`,
    `ActivitySnapshot`,
    `ActivityDetailSnapshot`,
    `ActivityRecordingSnapshot`,
    `ActivityLapSnapshot`,
    `ActivitySampleSnapshot`,
    `ActivityTimerEventSnapshot`,
    `ActivityTimerStateSnapshot`,
  }
}
```

## Detects an unreviewed public model

```rust
pub struct WireEnvelope {
    pub bytes: u64,
}
```

```rust
pub struct WireEnvelope {
    pub bytes: u64,
}
```

## Allows a reviewed public model

```rust
pub struct DeviceSnapshot {
    pub bytes: u64,
}
```
