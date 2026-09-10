use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{
    BackupDestination, DeviceDirectoryEntry, DeviceIoError, DeviceLink, DeviceLinkCapacity,
    DeviceLinkError, DeviceProbeRequest, DeviceRead, DeviceWrite,
};
use crate::{
    DeviceInventory, DevicePathInspection, DevicePathStatus, MountedMtpBackupProgress,
    MountedMtpUploadProgress, SafeRelativePath, TransportKind,
};
use garmin_progress::OperationStage;

/// Filesystem transport using the same object transaction as MTP.
pub struct DirectoryDevice {
    root: PathBuf,
}

impl DirectoryDevice {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    async fn resolve(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        create_parents: bool,
    ) -> Result<PathBuf, DeviceIoError> {
        if storage != "primary" {
            return Err(DeviceIoError::Storage(storage.to_owned()));
        }
        let mut current = self.root.clone();
        if !tokio::fs::symlink_metadata(&current)
            .await?
            .file_type()
            .is_dir()
        {
            return Err(DeviceIoError::UnsafePath(current.display().to_string()));
        }
        let parts = path.as_path().components().collect::<Vec<_>>();
        for (index, part) in parts.iter().enumerate() {
            let name = part.as_os_str().to_string_lossy();
            let mut entries = tokio::fs::read_dir(&current).await?;
            let mut found = None;
            while let Some(entry) = entries.next_entry().await? {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&name)
                {
                    if found.is_some() {
                        return Err(DeviceIoError::UnsafePath(path.to_string()));
                    }
                    found = Some(entry.path());
                }
            }
            current = found.unwrap_or_else(|| current.join(part.as_os_str()));
            let last = index + 1 == parts.len();
            match tokio::fs::symlink_metadata(&current).await {
                Ok(meta) if (last && meta.is_file()) || meta.is_dir() => {}
                Ok(_) => return Err(DeviceIoError::UnsafePath(path.to_string())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if !last && create_parents {
                        tokio::fs::create_dir(&current).await?;
                    } else {
                        for rest in &parts[index + 1..] {
                            current.push(rest.as_os_str());
                        }
                        return Ok(current);
                    }
                }
                Err(error) => return Err(error.into()),
            }
            if !last && !tokio::fs::symlink_metadata(&current).await?.is_dir() {
                return Err(DeviceIoError::UnsafePath(path.to_string()));
            }
        }
        Ok(current)
    }
}

#[async_trait::async_trait]
impl DeviceLink for DirectoryDevice {
    async fn capacity(&self) -> Result<Option<DeviceLinkCapacity>, DeviceLinkError> {
        Ok(Some(DeviceLinkCapacity {
            state: self.state().await?,
            storage_id: self.primary_storage_id().await?,
        }))
    }

    async fn probe(
        &self,
        request: &DeviceProbeRequest,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<crate::DeviceProbeReport, DeviceLinkError> {
        Ok(crate::probe_mass_storage_file(&self.root, &request.source, progress).await?)
    }
}

pub(super) async fn hash_file(path: &Path) -> Result<(u64, String), DeviceIoError> {
    if !tokio::fs::symlink_metadata(path).await?.is_file() {
        return Err(DeviceIoError::UnsafePath(path.display().to_string()));
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut hash = Sha256::new();
    let mut size = 0;
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hash.update(&buffer[..n]);
    }
    Ok((size, hex::encode(hash.finalize())))
}

pub(super) async fn verify_file(path: &Path, size: u64, sha256: &str) -> Result<(), DeviceIoError> {
    if hash_file(path).await? != (size, sha256.to_owned()) {
        return Err(DeviceIoError::Verification(path.display().to_string()));
    }
    Ok(())
}

#[async_trait::async_trait]
impl DeviceRead for DirectoryDevice {
    fn execution_target(&self) -> Option<&str> {
        None
    }
    async fn state(&self) -> Result<crate::DeviceStateSnapshot, DeviceIoError> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let metadata = std::fs::symlink_metadata(&root)?;
            if !metadata.is_dir() {
                return Err(DeviceIoError::UnsafePath(root.display().to_string()));
            }
            Ok(crate::DeviceStateSnapshot {
                storages: vec![crate::DeviceStorageState {
                    id: "primary".to_owned(),
                    label: "Device storage".to_owned(),
                    capacity: crate::filesystem_capacity(&root),
                    writable: metadata.permissions().readonly().then_some(false),
                }],
            })
        })
        .await
        .map_err(|error| DeviceIoError::Transport(error.to_string()))?
    }
    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, DeviceIoError> {
        let mut items = Vec::new();
        for path in paths {
            let status = self.inspect("primary", path).await?;
            items.push(DevicePathInspection {
                storage_id: "primary".to_owned(),
                storage_label: "Device storage".to_owned(),
                path: path.clone(),
                state: status.state(),
                size: status.size(),
            });
        }
        Ok(DeviceInventory {
            transport: TransportKind::MassStorage,
            paths: items,
        })
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        Ok("primary".to_owned())
    }
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        let resolved = self.resolve(storage, path, false).await?;
        match tokio::fs::symlink_metadata(resolved).await {
            Ok(meta) if meta.is_file() => Ok(DevicePathStatus::RegularFile { size: meta.len() }),
            Ok(meta) if meta.is_dir() => Ok(DevicePathStatus::Directory),
            Ok(_) => Ok(DevicePathStatus::Other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(DevicePathStatus::Missing)
            }
            Err(error) => Err(error.into()),
        }
    }
    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        let source_path = self.resolve(storage, path, false).await?;
        let mut source = tokio::fs::File::open(&source_path).await?;
        let (destination_path, destination) = destination.into_parts();
        let mut output = tokio::fs::File::from_std(destination);
        let mut hash = Sha256::new();
        let mut bytes = 0_u64;
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            if progress.reporter.is_cancelled() {
                return Err(DeviceIoError::Cancelled);
            }
            let n = source.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).await?;
            bytes += n as u64;
            hash.update(&buffer[..n]);
            progress.advanced(storage, path, bytes, size);
        }
        output.flush().await?;
        output.sync_all().await?;
        let digest = hex::encode(hash.finalize());
        if bytes != size {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
        verify_file(&destination_path, size, &digest).await?;
        progress.completed(storage, path, size);
        Ok(digest)
    }
    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        verify_file(&self.resolve(storage, path, false).await?, size, sha256).await
    }

    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        let resolved = self.resolve(storage, path, false).await?;
        let metadata = match tokio::fs::symlink_metadata(&resolved).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            return Err(DeviceIoError::UnsafePath(path.to_string()));
        }
        if metadata.len() > limit {
            return Err(DeviceIoError::LimitExceeded(path.to_string()));
        }
        let bytes = tokio::fs::read(resolved).await?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(DeviceIoError::LimitExceeded(path.to_string()));
        }
        Ok(Some(bytes))
    }

    async fn list_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<DeviceDirectoryEntry>, DeviceIoError> {
        let directory = self.resolve(storage, path, false).await?;
        let metadata = tokio::fs::symlink_metadata(&directory).await?;
        if !metadata.file_type().is_dir() {
            return Err(DeviceIoError::UnsafePath(path.to_string()));
        }
        let mut entries = tokio::fs::read_dir(directory).await?;
        let mut result = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(DeviceIoError::UnsafePath(path.to_string()));
            }
            let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
            let (state, size) = if metadata.file_type().is_file() {
                (crate::DevicePathState::RegularFile, Some(metadata.len()))
            } else if metadata.file_type().is_dir() {
                (crate::DevicePathState::Directory, None)
            } else {
                (crate::DevicePathState::Other, None)
            };
            result.push(DeviceDirectoryEntry { name, state, size });
        }
        result.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(result)
    }
}

#[async_trait::async_trait]
impl DeviceWrite for DirectoryDevice {
    async fn ensure_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        if storage != "primary" {
            return Err(DeviceIoError::Storage(storage.to_owned()));
        }
        let mut current = self.root.clone();
        for component in path.as_path().components() {
            let expected = component.as_os_str().to_string_lossy();
            let mut entries = tokio::fs::read_dir(&current).await?;
            let mut found = None;
            while let Some(entry) = entries.next_entry().await? {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected)
                {
                    if found.is_some() {
                        return Err(DeviceIoError::UnsafePath(path.to_string()));
                    }
                    found = Some(entry.path());
                }
            }
            current = found.unwrap_or_else(|| current.join(component.as_os_str()));
            match tokio::fs::symlink_metadata(&current).await {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => return Err(DeviceIoError::UnsafePath(path.to_string())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::fs::create_dir(&current).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    async fn create_verified_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceIoError> {
        let destination = self.resolve(storage, path, false).await?;
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    DeviceIoError::Occupied(path.to_string())
                } else {
                    error.into()
                }
            })?;
        let result = async {
            output.write_all(bytes).await?;
            output.flush().await?;
            output.sync_all().await?;
            let observed = tokio::fs::read(&destination).await?;
            if observed != bytes {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            Ok(())
        }
        .await;
        drop(output);
        if result.is_err() {
            let _ = tokio::fs::remove_file(&destination).await;
        }
        result
    }

    async fn remove_empty_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        let directory = self.resolve(storage, path, false).await?;
        let metadata = tokio::fs::symlink_metadata(&directory).await?;
        if !metadata.file_type().is_dir() {
            return Err(DeviceIoError::UnsafePath(path.to_string()));
        }
        tokio::fs::remove_dir(directory).await?;
        if self.inspect(storage, path).await? != DevicePathStatus::Missing {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
        Ok(())
    }

    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.verify(storage, path, size, sha256).await?;
        tokio::fs::remove_file(self.resolve(storage, path, false).await?).await?;
        if self.inspect(storage, path).await? != DevicePathStatus::Missing {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
        Ok(())
    }
    async fn delete_size_checked(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        let target = self.resolve(storage, path, false).await?;
        let metadata = tokio::fs::symlink_metadata(&target).await?;
        if !metadata.file_type().is_file() || metadata.len() != size {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
        tokio::fs::remove_file(target).await?;
        if self.inspect(storage, path).await? != DevicePathStatus::Missing {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
        Ok(())
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
        verify_file(source, size, sha256).await?;
        let destination = self.resolve(storage, path, true).await?;
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .await?;
        let result = async {
            let mut source = tokio::fs::File::open(source).await?;
            let mut buffer = vec![0; 1024 * 1024];
            let mut bytes = 0_u64;
            loop {
                if progress.reporter.is_cancelled() {
                    return Err(DeviceIoError::Cancelled);
                }
                let n = source.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                output.write_all(&buffer[..n]).await?;
                bytes += n as u64;
                progress.reporter.advanced_with_path(
                    OperationStage::Commit,
                    "Writing update file",
                    path.to_string(),
                    progress.completed_before + bytes,
                    Some(progress.total),
                );
            }
            output.flush().await?;
            output.sync_all().await?;
            verify_file(&destination, size, sha256).await
        }
        .await;
        drop(output);
        if result.is_err() {
            tokio::fs::remove_file(destination).await?;
        }
        result
    }
    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.upload(
            storage,
            path,
            backup,
            size,
            sha256,
            MountedMtpUploadProgress {
                reporter: garmin_progress::ProgressReporter::default(),
                completed_before: 0,
                total: size,
            },
        )
        .await
    }
}
