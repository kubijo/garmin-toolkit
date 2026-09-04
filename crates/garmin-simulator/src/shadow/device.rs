//! Preserve source metadata while MTP accounts for writes to the scoped shadow.

use super::Shadow;
use async_trait::async_trait;
use garmin_device::{
    DeviceInventory, DevicePathState, DeviceStateSnapshot, MountedMtpBackupProgress,
    MountedMtpUploadProgress, SafeRelativePath, StorageCapacity,
    storage::{BackupDestination, DeviceIoError, DeviceRead, DeviceWrite},
};
use std::path::Path;

#[async_trait]
impl DeviceRead for Shadow {
    fn execution_target(&self) -> Option<&str> {
        self.registered.device().execution_target()
    }

    async fn state(&self) -> Result<DeviceStateSnapshot, DeviceIoError> {
        let mut state = self.registered.device().state().await?;
        for storage in &mut state.storages {
            let source = self
                .snapshot
                .volumes
                .iter()
                .find(|volume| volume.source_id == storage.id)
                .ok_or_else(|| DeviceIoError::Storage(storage.id.clone()))?;
            storage.capacity = match (source.capacity.as_ref(), storage.capacity.bytes()) {
                (Some(capacity), Some((_, free))) => capacity.bytes().map_or_else(
                    || capacity.clone(),
                    |(total, _)| StorageCapacity::new(total, free),
                ),
                _ => StorageCapacity::unavailable("The capture has no usable storage metadata"),
            };
            storage.writable = source.writable;
        }
        Ok(state)
    }

    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, DeviceIoError> {
        self.registered.device().inventory(paths).await
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        self.registered.device().primary_storage_id().await
    }
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(DevicePathState, Option<u64>), DeviceIoError> {
        self.registered.device().inspect(storage, path).await
    }
    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        self.registered
            .device()
            .backup(storage, path, size, destination, progress)
            .await
    }
    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.registered
            .device()
            .verify(storage, path, size, sha256)
            .await
    }
}

#[async_trait]
impl DeviceWrite for Shadow {
    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.registered
            .device()
            .delete(storage, path, size, sha256)
            .await
    }
    async fn delete_unverified(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        self.registered
            .device()
            .delete_unverified(storage, path, size)
            .await
    }
    async fn upload(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        source: &Path,
        size: u64,
        sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), DeviceIoError> {
        self.registered
            .device()
            .upload(storage, path, source, size, sha256, progress)
            .await
    }
    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.registered
            .device()
            .restore(storage, path, size, backup, sha256)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::{Snapshot, Volume, register};
    use sha2::{Digest as _, Sha256};

    #[tokio::test]
    async fn scoped_shadow_preserves_total_unknowns_and_live_free_space() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("simulation/device/storage-001"))?;
        let snapshot = Snapshot {
            version: 1,
            execution_target: "metadata-test".to_owned(),
            source_device: "source".to_owned(),
            plan_digest: "plan".to_owned(),
            primary: "internal".to_owned(),
            files: vec![],
            volumes: vec![Volume {
                key: "storage-001".to_owned(),
                source_id: "internal".to_owned(),
                label: "Internal storage".to_owned(),
                capacity: Some(StorageCapacity::new(1_000, 100)),
                writable: Some(true),
            }],
        };
        let mut shadow = Shadow {
            registered: register(root.path(), &snapshot)?,
            snapshot,
        };
        assert_eq!(
            shadow.device().state().await?.storages[0].capacity.bytes(),
            Some((1_000, 100))
        );
        let payload = root.path().join("new.img");
        std::fs::write(&payload, b"new map")?;
        shadow
            .device()
            .upload(
                "internal",
                &SafeRelativePath::parse("new.img")?,
                &payload,
                7,
                &hex::encode(Sha256::digest(b"new map")),
                MountedMtpUploadProgress {
                    reporter: garmin_progress::ProgressReporter::default(),
                    completed_before: 0,
                    total: 7,
                },
            )
            .await?;
        assert_eq!(
            shadow.device().state().await?.storages[0].capacity.bytes(),
            Some((1_000, 93))
        );
        shadow.snapshot.volumes[0].capacity = Some(StorageCapacity::unavailable(
            "source did not report capacity",
        ));
        assert_eq!(
            shadow.device().state().await?.storages[0].capacity.bytes(),
            None
        );
        Ok(())
    }
}
