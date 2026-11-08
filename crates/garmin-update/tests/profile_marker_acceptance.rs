//! Explicitly invoked physical acceptance; ordinary tests use only disposable directories.

use std::error::Error;

use garmin_device::{
    MountedMtpDevice, SafeRelativePath, discover_mounted_mtp,
    storage::{DeviceRead, DeviceWrite, DirectoryDevice},
};
use garmin_model::identity::UserId;
use garmin_update::{
    PROFILE_MARKER_PATH, ProfileMarker, ProfileMarkerError, create_profile_marker,
    read_profile_marker, reassign_profile_marker,
};

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn directory_acceptance_preserves_history_and_restores_association() -> TestResult {
    let root = tempfile::tempdir()?;
    let device =
        DirectoryDevice::with_storage(root.path().to_path_buf(), "internal", "Synthetic storage");
    let marker = ProfileMarker::new(UserId::new_v4(), "Original synthetic profile")?;
    create_profile_marker(&device, "internal", &marker).await?;
    verify_reassignment(&device, "internal").await
}

/// Leaves two create-only revisions, with the original association effective again.
/// The owner must separately consent to those persistent history entries before invoking this test.
#[tokio::test]
#[ignore = "physical writes: requires owner consent, exact model selection, and exclusive device use"]
async fn physical_mounted_profile_reassignment() -> TestResult {
    let model = std::env::var("GARMIN_TOOLKIT_ACCEPTANCE_MODEL")?;
    let candidates = discover_mounted_mtp()
        .into_iter()
        .filter(|candidate| candidate.name == model)
        .collect::<Vec<_>>();
    let [candidate] = candidates.as_slice() else {
        return Err("expected exactly one mounted device matching the selected model".into());
    };
    let device = MountedMtpDevice::new(candidate.mount_id.clone());
    let manifest = device.open().await?;
    if manifest.summary.model != model {
        return Err("manifest model differs from the selected attachment".into());
    }
    let storage = device.primary_storage_id().await?;
    verify_reassignment(&device, &storage).await
}

async fn verify_reassignment<D: DeviceWrite>(device: &D, storage: &str) -> TestResult {
    let original = read_profile_marker(device, storage)
        .await?
        .ok_or("requires an existing pairing; first pairing is a separate acceptance case")?;
    if original.revision() > 62 {
        return Err("two free pairing revisions are required".into());
    }
    let mut history = Vec::new();
    for revision in 0..=original.revision() {
        let path = if revision == 0 {
            PROFILE_MARKER_PATH.to_owned()
        } else {
            format!("GARMIN-TOOLKIT/pairing-{revision:06}.toml")
        };
        let path = SafeRelativePath::parse(&path)?;
        let bytes = device
            .read_bounded_file(storage, &path, 16 * 1024)
            .await?
            .ok_or("pairing history changed before acceptance")?;
        history.push((path, bytes));
    }
    let temporary_user = UserId::new_v4();
    let conflict = ProfileMarker::new(temporary_user, "Synthetic acceptance profile")?;
    if !matches!(
        create_profile_marker(device, storage, &conflict).await,
        Err(ProfileMarkerError::Conflict)
    ) {
        return Err("existing pairing was not protected from conflicting creation".into());
    }
    let reassignment = reassign_profile_marker(
        device,
        storage,
        original.device_id(),
        original.revision(),
        temporary_user,
        "Synthetic acceptance profile",
    )
    .await;

    // Resolve even an ambiguous write before returning an error. Never restore over a third party's change.
    let current = read_profile_marker(device, storage)
        .await?
        .ok_or("pairing disappeared")?;
    let restored = if current.device_id() == original.device_id()
        && current.user_id() == temporary_user
        && current.revision() == original.revision() + 1
    {
        reassign_profile_marker(
            device,
            storage,
            current.device_id(),
            current.revision(),
            original.user_id(),
            original.profile_name(),
        )
        .await?
    } else if current.bytes()? == original.bytes()? {
        current
    } else {
        return Err("pairing changed unexpectedly; restore requires owner review".into());
    };
    if restored.device_id() != original.device_id()
        || restored.user_id() != original.user_id()
        || restored.profile_name() != original.profile_name()
    {
        return Err("original association was not restored".into());
    }
    for (path, bytes) in history {
        if device.read_bounded_file(storage, &path, 16 * 1024).await? != Some(bytes) {
            return Err("an original pairing document changed".into());
        }
    }
    let written = reassignment?;
    if written.device_id() != original.device_id()
        || written.user_id() != temporary_user
        || restored.revision() != original.revision() + 2
    {
        return Err("reassignment or restoration revision did not match".into());
    }
    println!(
        "Verified conflicting-create rejection, reassignment, restored association, and unchanged prior files."
    );
    Ok(())
}
