use std::{
    fs::File,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Poll,
    time::{Duration, Instant},
};

use bytes::Bytes;
use mtp_rs::{ByteRange, MtpDevice, NewObjectInfo, ObjectHandle, ObjectInfo, Storage};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncWriteExt as _, ReadBuf};

use super::{
    BackupDestination, DeviceDirectoryEntry, DeviceIoError, DeviceRead, DeviceWrite,
    directory::verify_file,
};
use crate::{
    DeviceInventory, DevicePathInspection, DevicePathState, DevicePathStatus,
    MountedMtpBackupProgress, MountedMtpUploadProgress, SafeRelativePath, TransportKind,
};
use garmin_progress::OperationStage;

#[derive(Clone)]
pub(crate) struct RawMtpUploadProgress {
    pub reporter: garmin_progress::ProgressReporter,
    pub stage: OperationStage,
    pub label: &'static str,
    pub completed_before: u64,
    pub total: u64,
    pub path: Option<String>,
    pub complete_payload_stage: bool,
}

pub(crate) enum RawMtpUploadOutcome {
    Acknowledged {
        handle: ObjectHandle,
        payload_elapsed: Duration,
        finalize_elapsed: Duration,
    },
    Ambiguous {
        handle: ObjectHandle,
        payload_elapsed: Duration,
        finalize_elapsed: Duration,
        error: mtp_rs::UploadError,
    },
    Failed {
        handle: Option<ObjectHandle>,
        error: mtp_rs::UploadError,
    },
}

pub(crate) async fn upload_mtp_file(
    storage: &Storage,
    parent: Option<ObjectHandle>,
    name: &str,
    source: tokio::fs::File,
    size: u64,
    progress: RawMtpUploadProgress,
) -> RawMtpUploadOutcome {
    report_started(
        &progress.reporter,
        progress.stage,
        progress.label,
        progress.path.as_deref(),
        Some(progress.total),
    );
    let started = Instant::now();
    let payload_elapsed = Arc::new(Mutex::new(None::<Duration>));
    let stream = payload_stream(
        source,
        size,
        started,
        Arc::clone(&payload_elapsed),
        progress.clone(),
    );
    let upload = storage.upload(parent, NewObjectInfo::file(name, size), stream);
    tokio::pin!(upload);
    let result = loop {
        tokio::select! {
            result = &mut upload => break result,
            () = tokio::time::sleep(Duration::from_secs(1)) => {
                let payload_elapsed = *payload_elapsed
                    .lock()
                    .expect("MTP payload clock poisoned");
                if let Some(payload_elapsed) = payload_elapsed {
                    report_advanced(
                        &progress.reporter,
                        OperationStage::DeviceFinalize,
                        &format!(
                            "Waiting for the device to finalize the MTP upload ({:.0}s elapsed)",
                            started.elapsed().saturating_sub(payload_elapsed).as_secs_f64()
                        ),
                        progress.path.as_deref(),
                        0,
                        1,
                    );
                }
            }
        }
    };
    let total_elapsed = started.elapsed();
    let payload_elapsed = *payload_elapsed.lock().expect("MTP payload clock poisoned");
    classify_upload(result, payload_elapsed, total_elapsed, size, &progress)
}

fn payload_stream(
    mut source: tokio::fs::File,
    size: u64,
    started: Instant,
    payload_elapsed: Arc<Mutex<Option<Duration>>>,
    progress: RawMtpUploadProgress,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Unpin {
    let RawMtpUploadProgress {
        reporter,
        stage,
        label,
        completed_before,
        total,
        path,
        complete_payload_stage,
    } = progress;
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    futures_util::stream::poll_fn(move |context| {
        if reporter.is_cancelled() {
            return Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "MTP upload cancelled",
            ))));
        }
        if copied == size {
            let mut payload_clock = payload_elapsed.lock().expect("MTP payload clock poisoned");
            if payload_clock.is_none() {
                *payload_clock = Some(started.elapsed());
                drop(payload_clock);
                if complete_payload_stage {
                    report_completed(
                        &reporter,
                        stage,
                        "MTP payload sent",
                        path.as_deref(),
                        completed_before.saturating_add(copied),
                        total,
                    );
                }
                report_started(
                    &reporter,
                    OperationStage::DeviceFinalize,
                    "Waiting for the device to finalize the MTP upload",
                    path.as_deref(),
                    None,
                );
            }
            return Poll::Ready(None);
        }

        // mtp-rs 0.32 sends Garmin's split header and payload chunks through
        // `send_bulk`, bypassing the streaming transport's terminating ZLP. Keep
        // one source byte for the final chunk so an exact USB packet-sized file
        // still ends in a short transfer. This preserves the declared object size
        // and works for production uploads as well as the disposable benchmark.
        let remaining = size - copied;
        let read_len = if remaining == 1 {
            1
        } else {
            let buffer_len = u64::try_from(buffer.len()).expect("buffer length fits in u64");
            usize::try_from((remaining - 1).min(buffer_len))
                .expect("read length is bounded by the host buffer")
        };
        let mut read_buffer = ReadBuf::new(&mut buffer[..read_len]);
        match Pin::new(&mut source).poll_read(context, &mut read_buffer) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Some(Err(error))),
            Poll::Ready(Ok(())) if read_buffer.filled().is_empty() => {
                Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("MTP upload source ended after {copied} of {size} bytes"),
                ))))
            }
            Poll::Ready(Ok(())) => {
                let chunk = Bytes::copy_from_slice(read_buffer.filled());
                let count = u64::try_from(chunk.len())
                    .map_err(std::io::Error::other)
                    .and_then(|count| {
                        copied
                            .checked_add(count)
                            .ok_or_else(|| std::io::Error::other("MTP upload byte count overflow"))
                    });
                match count {
                    Ok(count) => {
                        copied = count;
                        report_advanced(
                            &reporter,
                            stage,
                            label,
                            path.as_deref(),
                            completed_before.saturating_add(copied),
                            total,
                        );
                        Poll::Ready(Some(Ok(chunk)))
                    }
                    Err(error) => Poll::Ready(Some(Err(error))),
                }
            }
        }
    })
}

fn classify_upload(
    result: Result<ObjectHandle, mtp_rs::UploadError>,
    payload_elapsed: Option<Duration>,
    total_elapsed: Duration,
    size: u64,
    progress: &RawMtpUploadProgress,
) -> RawMtpUploadOutcome {
    match result {
        Ok(handle) => {
            let payload_elapsed = payload_elapsed.unwrap_or(total_elapsed);
            if !progress.complete_payload_stage {
                report_advanced(
                    &progress.reporter,
                    progress.stage,
                    progress.label,
                    progress.path.as_deref(),
                    progress.completed_before.saturating_add(size),
                    progress.total,
                );
            }
            report_completed(
                &progress.reporter,
                OperationStage::DeviceFinalize,
                "Device finalized the MTP upload",
                progress.path.as_deref(),
                1,
                1,
            );
            RawMtpUploadOutcome::Acknowledged {
                handle,
                payload_elapsed,
                finalize_elapsed: total_elapsed.saturating_sub(payload_elapsed),
            }
        }
        Err(error)
            if matches!(error.source, mtp_rs::Error::Timeout)
                && payload_elapsed.is_some()
                && error.partial.is_some() =>
        {
            let payload_elapsed = payload_elapsed.expect("checked above");
            report_advanced(
                &progress.reporter,
                OperationStage::DeviceFinalize,
                "Device response timed out; reconciling the uploaded object",
                progress.path.as_deref(),
                0,
                1,
            );
            RawMtpUploadOutcome::Ambiguous {
                handle: error.partial.expect("checked above"),
                payload_elapsed,
                finalize_elapsed: total_elapsed.saturating_sub(payload_elapsed),
                error,
            }
        }
        Err(error) => {
            progress.reporter.failed(
                if payload_elapsed.is_some() {
                    OperationStage::DeviceFinalize
                } else {
                    progress.stage
                },
                "MTP upload failed",
            );
            RawMtpUploadOutcome::Failed {
                handle: error.partial,
                error,
            }
        }
    }
}

fn report_started(
    reporter: &garmin_progress::ProgressReporter,
    stage: OperationStage,
    label: &str,
    path: Option<&str>,
    total: Option<u64>,
) {
    if let Some(path) = path {
        reporter.started_with_path(stage, label, path, total);
    } else {
        reporter.started(stage, label, total);
    }
}

fn report_advanced(
    reporter: &garmin_progress::ProgressReporter,
    stage: OperationStage,
    label: &str,
    path: Option<&str>,
    completed: u64,
    total: u64,
) {
    if let Some(path) = path {
        reporter.advanced_with_path(stage, label, path, completed, Some(total));
    } else {
        reporter.advanced(stage, label, completed, Some(total));
    }
}

fn report_completed(
    reporter: &garmin_progress::ProgressReporter,
    stage: OperationStage,
    label: &str,
    path: Option<&str>,
    completed: u64,
    total: u64,
) {
    if let Some(path) = path {
        reporter.completed_with_path(stage, label, path, completed, Some(total));
    } else {
        reporter.completed(stage, label, completed, Some(total));
    }
}

/// Stateful protocol adapter with caller-owned storage and execution identities.
pub struct MtpStorageDevice {
    location: u64,
    target: Option<String>,
    keys: Vec<String>,
    primary: String,
}

impl MtpStorageDevice {
    #[must_use]
    pub fn new(location: u64, target: Option<String>, keys: Vec<String>, primary: String) -> Self {
        Self {
            location,
            target,
            keys,
            primary,
        }
    }

    async fn connect(&self) -> Result<MtpDevice, DeviceIoError> {
        Ok(crate::mtp::open_raw_mtp(self.location).await?)
    }

    async fn storage(&self, device: &MtpDevice, key: &str) -> Result<Storage, DeviceIoError> {
        let index = self
            .keys
            .iter()
            .position(|k| k == key)
            .ok_or_else(|| DeviceIoError::Storage(key.to_owned()))?;
        device
            .storages()
            .await?
            .into_iter()
            .nth(index)
            .ok_or_else(|| DeviceIoError::Storage(key.to_owned()))
    }
}

async fn finish<T>(
    device: MtpDevice,
    result: Result<T, DeviceIoError>,
) -> Result<T, DeviceIoError> {
    let close = device.close().await;
    match (result, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Ok(())) => Err(error),
        (Err(operation), Err(cleanup)) => Err(DeviceIoError::Transport(format!(
            "{operation}; session cleanup also failed: {cleanup}"
        ))),
    }
}

async fn cleanup_failed_upload(
    storage: &Storage,
    path: &SafeRelativePath,
    handle: ObjectHandle,
    operation: DeviceIoError,
) -> DeviceIoError {
    if let Err(cleanup) = storage.delete(handle).await {
        return DeviceIoError::Transport(format!(
            "{operation}; partial-object cleanup failed: {cleanup}"
        ));
    }
    match find(storage, path).await {
        Ok(None) => operation,
        Ok(Some(_)) => DeviceIoError::Transport(format!(
            "{operation}; partial-object cleanup left {path} on the device"
        )),
        Err(cleanup) => DeviceIoError::Transport(format!(
            "{operation}; partial-object cleanup could not be verified: {cleanup}"
        )),
    }
}

async fn child(
    storage: &Storage,
    parent: Option<ObjectHandle>,
    name: &str,
) -> Result<Option<ObjectInfo>, DeviceIoError> {
    let mut matches = storage
        .list_objects(parent)
        .await?
        .into_iter()
        .filter(|o| o.filename.eq_ignore_ascii_case(name));
    let first = matches.next();
    if matches.next().is_some() {
        return Err(DeviceIoError::UnsafePath(name.to_owned()));
    }
    Ok(first)
}

async fn find(
    storage: &Storage,
    path: &SafeRelativePath,
) -> Result<Option<ObjectInfo>, DeviceIoError> {
    let parts = path.as_path().components().collect::<Vec<_>>();
    let mut parent = None;
    for (index, part) in parts.iter().enumerate() {
        let Some(object) = child(storage, parent, &part.as_os_str().to_string_lossy()).await?
        else {
            return Ok(None);
        };
        if index + 1 == parts.len() {
            return Ok(Some(object));
        }
        if !object.is_folder() {
            return Err(DeviceIoError::UnsafePath(path.to_string()));
        }
        parent = Some(object.handle);
    }
    Err(DeviceIoError::UnsafePath(path.to_string()))
}

async fn read(
    storage: &Storage,
    path: &SafeRelativePath,
    size: u64,
    output: Option<File>,
    progress: Option<(&str, MountedMtpBackupProgress)>,
) -> Result<String, DeviceIoError> {
    let object = find(storage, path)
        .await?
        .ok_or_else(|| DeviceIoError::Verification(path.to_string()))?;
    if !object.is_file() || object.size != size {
        return Err(DeviceIoError::Verification(path.to_string()));
    }
    let mut download = storage.download(object.handle, ByteRange::Full).await?;
    let mut output = output.map(tokio::fs::File::from_std);
    let mut bytes = 0_u64;
    let mut hash = Sha256::new();
    while let Some(chunk) = download.next_chunk().await {
        if let Some((_, progress)) = &progress
            && progress.reporter.is_cancelled()
        {
            download.cancel(mtp_rs::DEFAULT_CANCEL_TIMEOUT).await?;
            return Err(DeviceIoError::Cancelled);
        }
        let chunk = chunk?;
        bytes += chunk.len() as u64;
        if bytes > size {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
        hash.update(&chunk);
        if let Some(output) = &mut output {
            output.write_all(&chunk).await?;
        }
        if let Some((storage_id, progress)) = &progress {
            progress.advanced(storage_id, path, bytes, size);
        }
    }
    if let Some(output) = &mut output {
        output.flush().await?;
        output.sync_all().await?;
    }
    if bytes != size {
        return Err(DeviceIoError::Verification(path.to_string()));
    }
    if let Some((storage_id, progress)) = &progress {
        progress.completed(storage_id, path, size);
    }
    Ok(hex::encode(hash.finalize()))
}

#[async_trait::async_trait]
impl DeviceRead for MtpStorageDevice {
    fn execution_target(&self) -> Option<&str> {
        self.target.as_deref()
    }
    async fn state(&self) -> Result<crate::DeviceStateSnapshot, DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let mut storages = Vec::new();
            for (index, key) in self.keys.iter().enumerate() {
                let storage = self.storage(&device, key).await?;
                let info = storage.info();
                storages.push(crate::DeviceStorageState {
                    id: key.clone(),
                    label: if info.description.is_empty() {
                        format!("MTP storage {}", index + 1)
                    } else {
                        info.description.clone()
                    },
                    capacity: crate::StorageCapacity::new(info.total_capacity, info.free_space),
                    writable: Some(info.is_writable),
                });
            }
            Ok(crate::DeviceStateSnapshot { storages })
        }
        .await;
        finish(device, result).await
    }
    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let mut items = Vec::new();
            for key in &self.keys {
                let storage = self.storage(&device, key).await?;
                for path in paths {
                    let (state, size) = object_state(find(&storage, path).await?);
                    items.push(DevicePathInspection {
                        storage_id: key.clone(),
                        storage_label: storage.info().description.clone(),
                        path: path.clone(),
                        state,
                        size,
                    });
                }
            }
            Ok(DeviceInventory {
                transport: TransportKind::Mtp,
                paths: items,
            })
        }
        .await;
        finish(device, result).await
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        Ok(self.primary.clone())
    }
    async fn inspect(
        &self,
        key: &str,
        path: &SafeRelativePath,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let (state, size) = object_state(find(&self.storage(&device, key).await?, path).await?);
            DevicePathStatus::from_parts(state, size).ok_or_else(|| {
                DeviceIoError::Transport(format!(
                    "raw MTP returned inconsistent path metadata: {state:?}, size {size:?}"
                ))
            })
        }
        .await;
        finish(device, result).await
    }
    async fn backup(
        &self,
        key: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            let (destination_path, destination) = destination.into_parts();
            let hash = read(
                &storage,
                path,
                size,
                Some(destination),
                Some((key, progress)),
            )
            .await?;
            verify_file(&destination_path, size, &hash).await?;
            Ok(hash)
        }
        .await;
        finish(device, result).await
    }
    async fn verify(
        &self,
        key: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            if read(&self.storage(&device, key).await?, path, size, None, None).await? != sha256 {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            Ok(())
        }
        .await;
        finish(device, result).await
    }

    async fn read_bounded_file(
        &self,
        key: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            let Some(object) = find(&storage, path).await? else {
                return Ok(None);
            };
            if !object.is_file() {
                return Err(DeviceIoError::UnsafePath(path.to_string()));
            }
            if object.size > limit {
                return Err(DeviceIoError::LimitExceeded(path.to_string()));
            }
            let mut download = storage.download(object.handle, ByteRange::Full).await?;
            let mut bytes = Vec::with_capacity(usize::try_from(object.size).unwrap_or_default());
            while let Some(chunk) = download.next_chunk().await {
                let chunk = chunk?;
                if u64::try_from(bytes.len().saturating_add(chunk.len())).unwrap_or(u64::MAX)
                    > limit
                {
                    return Err(DeviceIoError::LimitExceeded(path.to_string()));
                }
                bytes.extend_from_slice(&chunk);
            }
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != object.size {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            Ok(Some(bytes))
        }
        .await;
        finish(device, result).await
    }

    async fn list_directory(
        &self,
        key: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<DeviceDirectoryEntry>, DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            let directory = find(&storage, path)
                .await?
                .filter(ObjectInfo::is_folder)
                .ok_or_else(|| DeviceIoError::UnsafePath(path.to_string()))?;
            let mut names = std::collections::BTreeSet::new();
            let mut entries = Vec::new();
            for object in storage.list_objects(Some(directory.handle)).await? {
                if !names.insert(object.filename.to_ascii_lowercase()) {
                    return Err(DeviceIoError::UnsafePath(path.to_string()));
                }
                let (state, size) = object_state(Some(object.clone()));
                entries.push(DeviceDirectoryEntry {
                    name: object.filename,
                    state,
                    size,
                });
            }
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(entries)
        }
        .await;
        finish(device, result).await
    }
}

fn object_state(object: Option<ObjectInfo>) -> (DevicePathState, Option<u64>) {
    match object {
        None => (DevicePathState::Missing, None),
        Some(object) if object.is_file() => (DevicePathState::RegularFile, Some(object.size)),
        Some(object) if object.is_folder() => (DevicePathState::Directory, None),
        Some(_) => (DevicePathState::Other, None),
    }
}

#[async_trait::async_trait]
impl DeviceWrite for MtpStorageDevice {
    async fn ensure_directory(
        &self,
        key: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            let mut parent = None;
            for part in path.as_path().components() {
                let name = part.as_os_str().to_string_lossy();
                parent = Some(match child(&storage, parent, &name).await? {
                    Some(object) if object.is_folder() => object.handle,
                    Some(_) => return Err(DeviceIoError::UnsafePath(path.to_string())),
                    None => storage.create_folder(parent, &name).await?,
                });
            }
            Ok(())
        }
        .await;
        finish(device, result).await
    }

    async fn create_verified_file(
        &self,
        key: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceIoError> {
        let temporary = tempfile::NamedTempFile::new()?;
        tokio::fs::write(temporary.path(), bytes).await?;
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(temporary.path())
            .await?;
        file.sync_all().await?;
        drop(file);
        let size = u64::try_from(bytes.len())
            .map_err(|_| DeviceIoError::LimitExceeded(path.to_string()))?;
        let sha256 = hex::encode(Sha256::digest(bytes));
        self.upload(
            key,
            path,
            temporary.path(),
            size,
            &sha256,
            MountedMtpUploadProgress {
                reporter: garmin_progress::ProgressReporter::default(),
                completed_before: 0,
                total: size,
            },
        )
        .await
    }

    async fn remove_empty_directory(
        &self,
        key: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            let directory = find(&storage, path)
                .await?
                .filter(ObjectInfo::is_folder)
                .ok_or_else(|| DeviceIoError::UnsafePath(path.to_string()))?;
            if !storage
                .list_objects(Some(directory.handle))
                .await?
                .is_empty()
            {
                return Err(DeviceIoError::Occupied(path.to_string()));
            }
            storage.delete(directory.handle).await?;
            if find(&storage, path).await?.is_some() {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            Ok(())
        }
        .await;
        finish(device, result).await
    }

    async fn delete(
        &self,
        key: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            if read(&storage, path, size, None, None).await? != sha256 {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            let object = find(&storage, path)
                .await?
                .ok_or_else(|| DeviceIoError::Verification(path.to_string()))?;
            storage.delete(object.handle).await?;
            if find(&storage, path).await?.is_some() {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            Ok(())
        }
        .await;
        finish(device, result).await
    }
    async fn delete_size_checked(
        &self,
        key: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            let object = find(&storage, path)
                .await?
                .filter(|object| object.is_file() && object.size == size)
                .ok_or_else(|| DeviceIoError::Verification(path.to_string()))?;
            storage.delete(object.handle).await?;
            if find(&storage, path).await?.is_some() {
                return Err(DeviceIoError::Verification(path.to_string()));
            }
            Ok(())
        }
        .await;
        finish(device, result).await
    }
    async fn upload(
        &self,
        key: &str,
        path: &SafeRelativePath,
        source: &Path,
        size: u64,
        sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), DeviceIoError> {
        verify_file(source, size, sha256).await?;
        let device = self.connect().await?;
        let result = async {
            let storage = self.storage(&device, key).await?;
            if find(&storage, path).await?.is_some() {
                return Err(DeviceIoError::Occupied(path.to_string()));
            }
            let mut parent = None;
            let parts = path.as_path().components().collect::<Vec<_>>();
            let (name, directories) = parts
                .split_last()
                .ok_or_else(|| DeviceIoError::UnsafePath(path.to_string()))?;
            for part in directories {
                let name = part.as_os_str().to_string_lossy();
                parent = Some(match child(&storage, parent, &name).await? {
                    Some(object) if object.is_folder() => object.handle,
                    Some(_) => return Err(DeviceIoError::UnsafePath(path.to_string())),
                    None => storage.create_folder(parent, &name).await?,
                });
            }
            let file = tokio::fs::File::open(source).await?;
            let display_path = path.to_string();
            let outcome = upload_mtp_file(
                &storage,
                parent,
                name.as_os_str().to_string_lossy().as_ref(),
                file,
                size,
                RawMtpUploadProgress {
                    reporter: progress.reporter.clone(),
                    stage: OperationStage::Commit,
                    label: "Writing update file",
                    completed_before: progress.completed_before,
                    total: progress.total,
                    path: Some(display_path),
                    complete_payload_stage: false,
                },
            )
            .await;
            let (handle, ambiguous) = match outcome {
                RawMtpUploadOutcome::Acknowledged { handle, .. } => (handle, false),
                RawMtpUploadOutcome::Ambiguous { handle, .. } => (handle, true),
                RawMtpUploadOutcome::Failed { handle, error } => {
                    let operation = if progress.reporter.is_cancelled() {
                        DeviceIoError::Cancelled
                    } else {
                        error.source.into()
                    };
                    return Err(match handle {
                        Some(partial) => {
                            cleanup_failed_upload(&storage, path, partial, operation).await
                        }
                        None => operation,
                    });
                }
            };
            match read(&storage, path, size, None, None).await {
                Ok(hash) if hash == sha256 => {
                    if ambiguous {
                        progress.reporter.completed_with_path(
                            OperationStage::DeviceFinalize,
                            "MTP upload reconciled by SHA-256 read-back",
                            path.to_string(),
                            1,
                            Some(1),
                        );
                    }
                    Ok(())
                }
                result => {
                    let operation = result
                        .err()
                        .unwrap_or_else(|| DeviceIoError::Verification(path.to_string()));
                    Err(cleanup_failed_upload(&storage, path, handle, operation).await)
                }
            }
        }
        .await;
        finish(device, result).await
    }
    async fn restore(
        &self,
        key: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.upload(
            key,
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt as _;
    use garmin_progress::{ProgressReporter, ProgressState};

    fn progress(reporter: ProgressReporter, total: u64) -> RawMtpUploadProgress {
        RawMtpUploadProgress {
            reporter,
            stage: OperationStage::Upload,
            label: "Uploading test payload",
            completed_before: 0,
            total,
            path: None,
            complete_payload_stage: true,
        }
    }

    #[test]
    fn timeout_after_payload_eof_is_an_ambiguous_finalization() {
        let (reporter, receiver) = ProgressReporter::channel();
        let outcome = classify_upload(
            Err(mtp_rs::UploadError {
                source: mtp_rs::Error::Timeout,
                partial: Some(ObjectHandle(7)),
            }),
            Some(Duration::from_secs(16)),
            Duration::from_secs(46),
            256,
            &progress(reporter, 256),
        );

        match outcome {
            RawMtpUploadOutcome::Ambiguous {
                payload_elapsed,
                finalize_elapsed,
                ..
            } => {
                assert_eq!(payload_elapsed, Duration::from_secs(16));
                assert_eq!(finalize_elapsed, Duration::from_secs(30));
            }
            _ => panic!("post-EOF timeout must require reconciliation"),
        }
        assert!(receiver.try_iter().any(|event| {
            event.stage == OperationStage::DeviceFinalize && event.state == ProgressState::Advanced
        }));
    }

    #[test]
    fn timeout_before_payload_eof_is_a_failed_upload() {
        let outcome = classify_upload(
            Err(mtp_rs::UploadError {
                source: mtp_rs::Error::Timeout,
                partial: Some(ObjectHandle(7)),
            }),
            None,
            Duration::from_secs(30),
            256,
            &progress(ProgressReporter::default(), 256),
        );

        assert!(matches!(outcome, RawMtpUploadOutcome::Failed { .. }));
    }

    #[tokio::test]
    async fn exact_packet_sized_payload_ends_with_a_short_chunk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("payload.bin");
        let expected = vec![0xA5; 1024];
        tokio::fs::write(&path, &expected).await.unwrap();
        let file = tokio::fs::File::open(path).await.unwrap();
        let mut stream = payload_stream(
            file,
            1024,
            Instant::now(),
            Arc::new(Mutex::new(None)),
            progress(ProgressReporter::default(), 1024),
        );
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.next().await {
            chunks.push(chunk.unwrap());
        }

        assert_eq!(chunks.last().map(Bytes::len), Some(1));
        assert_eq!(chunks.into_iter().flatten().collect::<Vec<_>>(), expected);
    }

    #[tokio::test]
    async fn changed_source_size_fails_instead_of_claiming_payload_completion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("short.bin");
        tokio::fs::write(&path, [1, 2, 3, 4]).await.unwrap();
        let file = tokio::fs::File::open(path).await.unwrap();
        let mut stream = payload_stream(
            file,
            5,
            Instant::now(),
            Arc::new(Mutex::new(None)),
            progress(ProgressReporter::default(), 5),
        );

        assert!(stream.next().await.unwrap().is_ok());
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
