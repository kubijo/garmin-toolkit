---
level: error
---

# Require review for public service models

Public service-boundary models must be explicitly reviewed and added to the approved model set.

The map workflow models use owned, Postcard-compatible values and fixed-width counts. Commands carry opaque request,
revision, and approval identities; host captures, transport handles, payloads, and raw progress events stay server-side.
`DeviceFitImportPlan` carries opaque review/job IDs and the existing owned FIT preview; device bytes stay server-side.
`DeviceFitImportStatus` carries only a fixed lifecycle state; the host retains the running job and stored acquisition.

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
    `DeviceFitImportPlan`,
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
