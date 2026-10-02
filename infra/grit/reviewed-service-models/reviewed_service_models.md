---
level: error
---

# Require review for public service models

Public service-boundary models must be explicitly reviewed and added to the approved model set.

The map workflow models use owned, Postcard-compatible values and fixed-width counts. Commands carry opaque request,
revision, and approval identities; host captures, transport handles, payloads, and raw progress events stay server-side.

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
    `DeviceFitPreviewActivity`,
    `DeviceFitImportOutcome`,
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
