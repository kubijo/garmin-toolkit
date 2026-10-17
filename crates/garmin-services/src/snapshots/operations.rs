use garmin_progress::CancellationToken;
use std::collections::HashMap;
use std::io;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use garmin_model::identity::UserId;
use garmin_service_api::snapshots::{
    MAX_SNAPSHOT_CHUNK, SnapshotFailure, SnapshotFailureKind as FailureKind, SnapshotOperation,
    SnapshotPreview, SnapshotReply, SnapshotRequest, SnapshotSource, SnapshotState, SnapshotStatus,
};
use garmin_storage::snapshot::{Limits, PreparedRestore};
use tempfile::TempDir;
use uuid::Uuid;

use crate::{UserContext, deployment::Deployment};

#[cfg(test)]
mod tests;
mod worker;

const MAX_OPERATIONS: usize = 16;
type Result<T> = std::result::Result<T, SnapshotFailure>;

struct Entry {
    instance: Uuid,
    actor: UserId,
    session: Uuid,
    connection: Weak<()>,
    source: SnapshotSource,
    status: SnapshotStatus,
    directory: Option<Arc<TempDir>>,
    file: Option<File>,
    prepared: Option<PreparedRestore>,
    cancel: CancellationToken,
}

impl Entry {
    fn discard(&mut self) {
        self.file = None;
        self.prepared = None;
        self.directory = None;
        self.status.preview = None;
    }
}

/// Bounded host-owned operations. No request can supply a filesystem path.
pub struct SnapshotOperations {
    deployment: Arc<Deployment>,
    limits: Limits,
    entries: Mutex<HashMap<Uuid, Entry>>,
}

impl SnapshotOperations {
    /// Creates the operation registry with validated transfer and database limits.
    /// # Errors
    /// Rejects unsupported limits before accepting any upload bytes.
    pub fn new(deployment: Arc<Deployment>, limits: Limits) -> Result<Arc<Self>> {
        limits
            .validate()
            .map_err(|error| failure(FailureKind::Limit, error))?;
        Ok(Arc::new(Self {
            deployment,
            limits,
            entries: Mutex::new(HashMap::new()),
        }))
    }

    /// Binds a connection to the host-supplied actor and current database epoch.
    /// # Errors
    /// Missing/member profiles and closed deployments are rejected.
    pub async fn connect(self: &Arc<Self>, actor: UserContext) -> Result<SnapshotSession> {
        let epoch = self.deployment.epoch();
        self.deployment
            .application(epoch)
            .await
            .map_err(stale)?
            .require_snapshot_owner(actor)
            .await
            .map_err(|error| failure(FailureKind::Unauthorized, error))?;
        Ok(SnapshotSession {
            operations: Arc::clone(self),
            actor,
            epoch,
            session: Uuid::new_v4(),
            connection: Arc::new(()),
            outcome_only: None,
        })
    }

    /// Reconnects an operation, retaining access to its outcome if restore removed its actor.
    /// # Errors
    /// Rejects unknown operations and actors that did not start the operation.
    pub async fn reconnect(
        self: &Arc<Self>,
        actor: UserContext,
        operation: Uuid,
    ) -> Result<SnapshotSession> {
        let epoch = self.deployment.epoch();
        let outcome_only = {
            let entries = self.entries();
            let entry = entries
                .get(&operation)
                .ok_or_else(|| failure(FailureKind::NotFound, "unknown snapshot operation"))?;
            if entry.actor != actor.user_id() {
                return Err(failure(
                    FailureKind::Unauthorized,
                    "snapshot belongs to another actor",
                ));
            }
            entry.status.epoch != epoch
                && (terminal(entry.status.state) || entry.status.state == SnapshotState::Restoring)
                || entry.status.state == SnapshotState::Completed
        };
        if !outcome_only {
            return self.connect(actor).await;
        }
        Ok(SnapshotSession {
            operations: Arc::clone(self),
            actor,
            epoch,
            session: Uuid::new_v4(),
            connection: Arc::new(()),
            outcome_only: Some(operation),
        })
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<Uuid, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One authenticated client connection. Reconnect uses `Resume`, which revokes earlier approvals.
pub struct SnapshotSession {
    operations: Arc<SnapshotOperations>,
    actor: UserContext,
    epoch: Uuid,
    session: Uuid,
    connection: Arc<()>,
    outcome_only: Option<Uuid>,
}

/// A host download bound to one operation instance, even if its request ID is reused.
pub struct SnapshotDownload {
    session: Arc<SnapshotSession>,
    operation: Uuid,
    instance: Uuid,
    size: u64,
}

impl SnapshotDownload {
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Reads a bounded chunk from the original operation.
    /// # Errors
    /// Rejects revoked sessions, released operations, and invalid read bounds.
    pub async fn read(&self, offset: u64, max_bytes: u32) -> Result<SnapshotReply> {
        self.session.authorize().await?;
        self.session
            .read(self.operation, offset, max_bytes, Some(self.instance))
    }

    /// Releases the original operation after delivery, expiry, or an abandoned response.
    /// # Errors
    /// Rejects a revoked session or an operation that has already been released or replaced.
    pub fn release(&self) -> Result<SnapshotReply> {
        self.session.release(self.operation, Some(self.instance))
    }
}

impl SnapshotSession {
    /// Binds a host download to the current ready snapshot without transferring session ownership.
    /// # Errors
    /// Rejects stale sessions, unauthorized actors, and snapshots that are not ready.
    pub async fn download(self: &Arc<Self>, operation: Uuid) -> Result<SnapshotDownload> {
        self.authorize().await?;
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        require_state(entry, SnapshotState::DownloadReady)?;
        Ok(SnapshotDownload {
            session: Arc::clone(self),
            operation,
            instance: entry.instance,
            size: entry.status.total.ok_or_else(missing_file)?,
        })
    }

    /// Starts a host-local file operation using the same cancellation and approval lifecycle.
    /// # Errors
    /// Rejects unauthorized actors, invalid paths, limits, and managed database destinations.
    pub async fn file(
        &self,
        operation: Uuid,
        selection: garmin_service_api::files::Selection,
    ) -> Result<SnapshotStatus> {
        self.authorize().await?;
        let path = std::path::PathBuf::from(selection.path);
        if !path.is_absolute() || !path.to_string_lossy().ends_with(".tar.zst") {
            return Err(failure(
                FailureKind::InvalidState,
                "select an absolute .tar.zst file path",
            ));
        }
        let cancel = CancellationToken::default();
        let reply = match selection.operation {
            garmin_service_api::files::Operation::Save => {
                let parent = path
                    .parent()
                    .ok_or_else(missing_file)?
                    .canonicalize()
                    .map_err(io_failure)?;
                if parent.starts_with(
                    self.operations
                        .deployment
                        .root()
                        .canonicalize()
                        .map_err(io_failure)?,
                ) {
                    return Err(failure(
                        FailureKind::InvalidState,
                        "Save the backup outside the application's data directory",
                    ));
                }
                let path = parent.join(path.file_name().ok_or_else(missing_file)?);
                if path.exists() && !selection.replace {
                    return Err(failure(
                        FailureKind::InvalidState,
                        "confirm replacement of the existing backup",
                    ));
                }
                self.begin(
                    operation,
                    None,
                    SnapshotSource::ServerSave(path.to_string_lossy().into_owned()),
                    Some(worker::Destination {
                        path,
                        replace: selection.replace,
                    }),
                    cancel,
                )?
            }
            garmin_service_api::files::Operation::Open => {
                if !path.metadata().map_err(io_failure)?.is_file() {
                    return Err(failure(FailureKind::InvalidState, "select a regular file"));
                }
                let source = SnapshotSource::ServerRestore(path.to_string_lossy().into_owned());
                let input = File::open(path).map_err(io_failure)?;
                let metadata = input.metadata().map_err(io_failure)?;
                if !metadata.is_file() {
                    return Err(failure(FailureKind::InvalidState, "select a regular file"));
                }
                let reply = self.begin(
                    operation,
                    Some(metadata.len()),
                    source,
                    None,
                    cancel.clone(),
                )?;
                worker::import_file(Arc::clone(&self.operations), operation, input, cancel);
                reply
            }
        };
        match reply {
            SnapshotReply::Status(status) => Ok(status),
            _ => Err(failure(FailureKind::InvalidState, "snapshot did not start")),
        }
    }

    /// Executes a bounded request. Long work returns a pollable operation immediately.
    /// # Errors
    /// Authorization, stale session/epoch, limits, invalid transitions, and I/O failures are typed.
    pub async fn execute(&self, request: SnapshotRequest) -> Result<SnapshotReply> {
        if let Some(allowed) = self.outcome_only {
            match &request {
                SnapshotRequest::Status { operation }
                | SnapshotRequest::Resume { operation }
                | SnapshotRequest::Recover { operation }
                | SnapshotRequest::Cancel { operation }
                | SnapshotRequest::Release { operation }
                    if *operation == allowed => {}
                _ => {
                    return Err(failure(
                        FailureKind::Unauthorized,
                        "reconnected session can only read and release this outcome",
                    ));
                }
            }
        }
        match request {
            SnapshotRequest::Status { operation } => {
                return self.status(operation, false, false).await;
            }
            SnapshotRequest::Resume { operation } => {
                return self.status(operation, true, false).await;
            }
            SnapshotRequest::Recover { operation } => {
                return self.status(operation, true, true).await;
            }
            SnapshotRequest::Cancel { operation } => return self.cancel(operation),
            SnapshotRequest::Release { operation } => return self.release(operation, None),
            _ => {}
        }
        self.authorize().await?;
        match request {
            SnapshotRequest::List => {
                let mut statuses: Vec<_> = self
                    .operations
                    .entries()
                    .values()
                    .filter(|entry| entry.actor == self.actor.user_id())
                    .map(|entry| {
                        let mut status = entry.status.clone();
                        status.preview = None;
                        SnapshotOperation {
                            status,
                            source: entry.source.clone(),
                            active: entry.connection.upgrade().is_some(),
                        }
                    })
                    .collect();
                statuses.sort_by_key(|entry| entry.status.operation);
                Ok(SnapshotReply::Operations(statuses))
            }
            SnapshotRequest::BeginBackup { operation } => self.begin(
                operation,
                None,
                SnapshotSource::Download,
                None,
                CancellationToken::default(),
            ),
            SnapshotRequest::BeginRestore { operation, bytes } => self.begin(
                operation,
                Some(bytes),
                SnapshotSource::Upload,
                None,
                CancellationToken::default(),
            ),
            SnapshotRequest::Upload {
                operation,
                offset,
                bytes,
            } => self.upload(operation, offset, &bytes),
            SnapshotRequest::Verify { operation } => self.verify(operation),
            SnapshotRequest::Approve {
                operation,
                approval,
            } => self.approve(operation, approval),
            SnapshotRequest::Read {
                operation,
                offset,
                max_bytes,
            } => self.read(operation, offset, max_bytes, None),
            _ => Err(failure(
                FailureKind::InvalidState,
                "request was already handled",
            )),
        }
    }

    async fn authorize(&self) -> Result<()> {
        if self.outcome_only.is_some() {
            return Err(failure(
                FailureKind::Unauthorized,
                "session is restricted to a restore outcome",
            ));
        }
        self.operations
            .deployment
            .application(self.epoch)
            .await
            .map_err(stale)?
            .require_snapshot_owner(self.actor)
            .await
            .map_err(|error| failure(FailureKind::Unauthorized, error))
    }

    fn entry<'a>(
        &self,
        entries: &'a mut HashMap<Uuid, Entry>,
        operation: Uuid,
    ) -> Result<&'a mut Entry> {
        let entry = entries
            .get_mut(&operation)
            .ok_or_else(|| failure(FailureKind::NotFound, "unknown snapshot operation"))?;
        if entry.actor != self.actor.user_id() {
            return Err(failure(
                FailureKind::Unauthorized,
                "snapshot belongs to another actor",
            ));
        }
        if entry.session != self.session {
            return Err(failure(
                FailureKind::Stale,
                "snapshot belongs to another connection; resume it first",
            ));
        }
        Ok(entry)
    }

    fn begin(
        &self,
        operation: Uuid,
        upload: Option<u64>,
        source: SnapshotSource,
        destination: Option<worker::Destination>,
        cancel: CancellationToken,
    ) -> Result<SnapshotReply> {
        if upload.is_some_and(|size| size == 0 || size > self.operations.limits.compressed_bytes) {
            return Err(failure(
                FailureKind::Limit,
                "snapshot upload exceeds its size limit",
            ));
        }
        let mut entries = self.operations.entries();
        if entries.contains_key(&operation) {
            return Err(failure(
                FailureKind::InvalidState,
                "operation already exists; resume it",
            ));
        }
        if entries.len() >= MAX_OPERATIONS {
            return Err(failure(
                FailureKind::Limit,
                "release an existing snapshot operation first",
            ));
        }
        let staging = self.operations.deployment.root().join(".tmp/snapshots");
        std::fs::create_dir_all(&staging).map_err(io_failure)?;
        let directory = Arc::new(tempfile::tempdir_in(staging).map_err(io_failure)?);
        let file = if upload.is_some() {
            Some(File::create(directory.path().join("upload.tar.zst")).map_err(io_failure)?)
        } else {
            None
        };
        let status = SnapshotStatus {
            operation,
            epoch: self.epoch,
            state: if upload.is_some() {
                SnapshotState::Uploading
            } else {
                SnapshotState::BackingUp
            },
            transferred: 0,
            total: upload,
            preview: None,
            error: None,
        };
        entries.insert(
            operation,
            Entry {
                instance: Uuid::new_v4(),
                actor: self.actor.user_id(),
                session: self.session,
                connection: Arc::downgrade(&self.connection),
                source,
                status: status.clone(),
                directory: Some(Arc::clone(&directory)),
                file,
                prepared: None,
                cancel: cancel.clone(),
            },
        );
        drop(entries);
        if upload.is_none() {
            worker::backup(
                Arc::clone(&self.operations),
                operation,
                self.epoch,
                self.actor,
                directory,
                cancel,
                destination,
            );
        }
        Ok(SnapshotReply::Status(status))
    }

    fn upload(&self, operation: Uuid, offset: u64, bytes: &[u8]) -> Result<SnapshotReply> {
        if bytes.is_empty() || bytes.len() > MAX_SNAPSHOT_CHUNK as usize {
            return Err(failure(FailureKind::Limit, "invalid snapshot chunk size"));
        }
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        require_state(entry, SnapshotState::Uploading)?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| failure(FailureKind::Limit, "snapshot offset overflow"))?;
        if offset != entry.status.transferred || end > entry.status.total.unwrap_or(0) {
            return Err(failure(
                FailureKind::Limit,
                "snapshot chunks must be contiguous and fit the declared length",
            ));
        }
        if let Err(error) = entry
            .file
            .as_mut()
            .ok_or_else(missing_file)?
            .write_all(bytes)
        {
            worker::fail(entry, error.to_string());
            return Err(io_failure(error));
        }
        entry.status.transferred = end;
        Ok(SnapshotReply::Status(entry.status.clone()))
    }

    fn verify(&self, operation: Uuid) -> Result<SnapshotReply> {
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        require_state(entry, SnapshotState::Uploading)?;
        if Some(entry.status.transferred) != entry.status.total {
            return Err(failure(
                FailureKind::InvalidState,
                "snapshot upload is incomplete",
            ));
        }
        if let Err(error) = entry.file.take().ok_or_else(missing_file)?.sync_all() {
            worker::fail(entry, error.to_string());
            return Err(io_failure(error));
        }
        let directory = entry.directory.as_ref().ok_or_else(missing_file)?.clone();
        entry.status.state = SnapshotState::Verifying;
        let status = entry.status.clone();
        let cancel = entry.cancel.clone();
        drop(entries);
        worker::verify(Arc::clone(&self.operations), operation, directory, cancel);
        Ok(SnapshotReply::Status(status))
    }

    fn approve(&self, operation: Uuid, approval: Uuid) -> Result<SnapshotReply> {
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        require_state(entry, SnapshotState::AwaitingApproval)?;
        if entry.status.epoch != self.epoch
            || entry
                .status
                .preview
                .as_ref()
                .is_none_or(|preview| preview.approval != approval)
        {
            return Err(failure(
                FailureKind::InvalidApproval,
                "restore approval is stale or does not match this snapshot",
            ));
        }
        let prepared = entry.prepared.take().ok_or_else(missing_file)?;
        entry.status.state = SnapshotState::Restoring;
        entry.status.preview = None;
        let status = entry.status.clone();
        drop(entries);
        worker::restore(
            Arc::clone(&self.operations),
            operation,
            self.epoch,
            self.actor,
            prepared,
        );
        Ok(SnapshotReply::Status(status))
    }

    async fn status(
        &self,
        operation: Uuid,
        resume: bool,
        abandoned_only: bool,
    ) -> Result<SnapshotReply> {
        if resume && self.outcome_only != Some(operation) {
            self.authorize().await?;
        }
        let current_epoch = self.operations.deployment.epoch();
        let mut entries = self.operations.entries();
        if resume {
            let entry = entries
                .get_mut(&operation)
                .ok_or_else(|| failure(FailureKind::NotFound, "unknown snapshot operation"))?;
            if entry.actor != self.actor.user_id() {
                return Err(failure(
                    FailureKind::Unauthorized,
                    "snapshot belongs to another actor",
                ));
            }
            if abandoned_only && entry.connection.upgrade().is_some() {
                return Err(failure(
                    FailureKind::InvalidState,
                    "snapshot is still in use by another connection or download",
                ));
            }
            entry.session = self.session;
            entry.connection = Arc::downgrade(&self.connection);
            if let Some(preview) = &mut entry.status.preview {
                preview.approval = Uuid::new_v4();
            }
        }
        let entry = self.entry(&mut entries, operation)?;
        if entry.status.epoch != current_epoch
            && !terminal(entry.status.state)
            && entry.status.state != SnapshotState::Restoring
        {
            entry.cancel.cancel();
            entry.discard();
            entry.status.state = SnapshotState::Failed;
            entry.status.error = Some("database changed; start a new operation".into());
        }
        Ok(SnapshotReply::Status(entry.status.clone()))
    }

    fn cancel(&self, operation: Uuid) -> Result<SnapshotReply> {
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        if entry.status.state == SnapshotState::Restoring {
            return Err(failure(
                FailureKind::InvalidState,
                "approved restore cannot be cancelled during database switching",
            ));
        }
        if !terminal(entry.status.state) {
            entry.cancel.cancel();
            entry.discard();
            entry.status.state = SnapshotState::Cancelled;
        }
        Ok(SnapshotReply::Status(entry.status.clone()))
    }

    fn release(&self, operation: Uuid, instance: Option<Uuid>) -> Result<SnapshotReply> {
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        require_instance(entry, instance)?;
        if !terminal(entry.status.state) && entry.status.state != SnapshotState::DownloadReady {
            return Err(failure(
                FailureKind::InvalidState,
                "cancel or finish the operation before releasing it",
            ));
        }
        // Any detached work must remain cancelled even if this ID is reused immediately.
        entry.cancel.cancel();
        entries.remove(&operation);
        Ok(SnapshotReply::Released)
    }

    fn read(
        &self,
        operation: Uuid,
        offset: u64,
        max_bytes: u32,
        instance: Option<Uuid>,
    ) -> Result<SnapshotReply> {
        if max_bytes == 0 || max_bytes > MAX_SNAPSHOT_CHUNK {
            return Err(failure(FailureKind::Limit, "invalid download chunk size"));
        }
        let mut entries = self.operations.entries();
        let entry = self.entry(&mut entries, operation)?;
        require_instance(entry, instance)?;
        require_state(entry, SnapshotState::DownloadReady)?;
        if entry.status.epoch != self.epoch {
            return Err(failure(FailureKind::Stale, "database changed"));
        }
        let file = entry.file.as_mut().ok_or_else(missing_file)?;
        let total = file.metadata().map_err(io_failure)?.len();
        if offset > total {
            return Err(failure(
                FailureKind::Limit,
                "download offset exceeds snapshot length",
            ));
        }
        file.seek(SeekFrom::Start(offset)).map_err(io_failure)?;
        let mut bytes = vec![
            0;
            usize::try_from((total - offset).min(u64::from(max_bytes)))
                .map_err(io_failure)?
        ];
        file.read_exact(&mut bytes).map_err(io_failure)?;
        let end = offset + bytes.len() as u64;
        entry.status.transferred = entry.status.transferred.max(end);
        Ok(SnapshotReply::Chunk {
            offset,
            bytes,
            end: end == total,
        })
    }
}

impl garmin_service_api::snapshots::SnapshotService for SnapshotSession {
    async fn execute(
        &self,
        request: SnapshotRequest,
    ) -> std::result::Result<Result<SnapshotReply>, remoc::rtc::CallError> {
        Ok(Self::execute(self, request).await)
    }
}

fn terminal(state: SnapshotState) -> bool {
    matches!(
        state,
        SnapshotState::Completed | SnapshotState::Cancelled | SnapshotState::Failed
    )
}
fn require_instance(entry: &Entry, instance: Option<Uuid>) -> Result<()> {
    if instance.is_some_and(|instance| instance != entry.instance) {
        return Err(failure(
            FailureKind::Stale,
            "snapshot operation was replaced",
        ));
    }
    Ok(())
}
fn require_state(entry: &Entry, state: SnapshotState) -> Result<()> {
    if entry.status.state == state {
        Ok(())
    } else {
        Err(failure(
            FailureKind::InvalidState,
            "snapshot operation is in a different state",
        ))
    }
}
fn missing_file() -> SnapshotFailure {
    failure(
        FailureKind::InvalidState,
        "snapshot data is no longer available",
    )
}
fn failure(kind: FailureKind, message: impl std::fmt::Display) -> SnapshotFailure {
    SnapshotFailure {
        kind,
        message: message.to_string(),
    }
}
fn io_failure(error: impl std::fmt::Display) -> SnapshotFailure {
    failure(FailureKind::Io, error)
}
fn stale(error: impl std::fmt::Display) -> SnapshotFailure {
    failure(FailureKind::Stale, error)
}
