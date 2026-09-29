use super::{
    Arc, CancellationToken, File, PreparedRestore, Read, SnapshotOperations, SnapshotPreview,
    SnapshotState, TempDir, UserContext, Uuid, io,
};
use std::{io::Write as _, path::PathBuf};

pub(super) struct Destination {
    pub path: PathBuf,
    pub replace: bool,
}

impl Destination {
    fn publish(self, output: tempfile::NamedTempFile) -> Result<(), String> {
        let parent = self.path.parent().ok_or("missing destination directory")?;
        if self.replace {
            output.persist(&self.path)
        } else {
            output.persist_noclobber(&self.path)
        }
        .map_err(|error| error.to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

enum Output {
    Download(File, u64),
    Saved(tempfile::NamedTempFile, Destination, u64),
}

pub(super) fn backup(
    operations: Arc<SnapshotOperations>,
    operation: Uuid,
    epoch: Uuid,
    actor: UserContext,
    directory: Arc<TempDir>,
    cancel: CancellationToken,
    destination: Option<Destination>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let runtime = tokio::runtime::Handle::current();
        let service = Arc::clone(&operations);
        let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || {
            if worker_cancel.is_cancelled() {
                return Err("backup cancelled".to_owned());
            }
            let path = directory.path().join("snapshot.tar.zst");
            runtime
                .block_on(service.deployment.backup(
                    epoch,
                    actor,
                    &path,
                    service.limits,
                    &worker_cancel,
                ))
                .map_err(|error| error.to_string())?;
            let file = File::open(path).map_err(|error| error.to_string())?;
            let length = file.metadata().map_err(|error| error.to_string())?.len();
            if let Some(destination) = destination {
                let parent = destination
                    .path
                    .parent()
                    .ok_or("missing destination directory")?;
                let mut output =
                    tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
                io::copy(
                    &mut CancelRead {
                        input: file,
                        cancel: worker_cancel,
                    },
                    &mut output,
                )
                .map_err(|error| error.to_string())?;
                output
                    .as_file()
                    .sync_all()
                    .map_err(|error| error.to_string())?;
                Ok(Output::Saved(output, destination, length))
            } else {
                Ok(Output::Download(file, length))
            }
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(std::convert::identity);
        let mut entries = operations.entries();
        let Some(entry) = entries.get_mut(&operation) else {
            return;
        };
        if cancel.is_cancelled() {
            return;
        }
        match result {
            Ok(Output::Download(file, length)) => {
                entry.file = Some(file);
                entry.status.total = Some(length);
                entry.status.state = SnapshotState::DownloadReady;
            }
            Ok(Output::Saved(output, destination, length)) => {
                let result = destination.publish(output);
                match result {
                    Ok(()) => {
                        entry.status.transferred = length;
                        entry.status.total = Some(length);
                        entry.status.state = SnapshotState::Completed;
                        entry.discard();
                    }
                    Err(error) => fail(entry, error),
                }
            }
            Err(error) => fail(entry, error),
        }
    })
}

pub(super) fn import_file(
    operations: Arc<SnapshotOperations>,
    operation: Uuid,
    mut input: File,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let service = Arc::clone(&operations);
        let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let (mut output, directory, total) = {
                let mut entries = service.entries();
                if worker_cancel.is_cancelled() {
                    return Err("backup reading cancelled".into());
                }
                let entry = entries
                    .get_mut(&operation)
                    .ok_or("snapshot operation was released")?;
                (
                    entry.file.take().ok_or("missing upload file")?,
                    entry.directory.clone().ok_or("missing staging directory")?,
                    entry.status.total.ok_or("missing file length")?,
                )
            };
            let mut bytes = vec![0; super::MAX_SNAPSHOT_CHUNK as usize];
            let mut transferred = 0;
            loop {
                if worker_cancel.is_cancelled() {
                    return Err("backup reading cancelled".into());
                }
                let count = input.read(&mut bytes).map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                transferred += count as u64;
                if transferred > total {
                    return Err("backup file changed while reading".into());
                }
                output
                    .write_all(&bytes[..count])
                    .map_err(|error| error.to_string())?;
                let mut entries = service.entries();
                if worker_cancel.is_cancelled() {
                    return Err("backup reading cancelled".into());
                }
                if let Some(entry) = entries.get_mut(&operation) {
                    entry.status.transferred = transferred;
                }
            }
            if transferred != total {
                return Err("backup file changed while reading".into());
            }
            output.sync_all().map_err(|error| error.to_string())?;
            Ok(directory)
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(std::convert::identity);
        {
            let mut entries = operations.entries();
            let Some(entry) = entries.get_mut(&operation) else {
                return;
            };
            if cancel.is_cancelled() {
                return;
            }
            match &result {
                Ok(_) => entry.status.state = SnapshotState::Verifying,
                Err(error) => fail(entry, error.clone()),
            }
        }
        if let Ok(directory) = result {
            verify(operations, operation, directory, cancel);
        }
    })
}

pub(super) fn verify(
    operations: Arc<SnapshotOperations>,
    operation: Uuid,
    directory: Arc<TempDir>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let runtime = tokio::runtime::Handle::current();
        let limits = operations.limits;
        let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || {
            let input = File::open(directory.path().join("upload.tar.zst"))
                .map_err(|error| error.to_string())?;
            runtime
                .block_on(PreparedRestore::read(
                    CancelRead {
                        input,
                        cancel: worker_cancel,
                    },
                    directory.path(),
                    limits,
                ))
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(std::convert::identity);
        let mut entries = operations.entries();
        let Some(entry) = entries.get_mut(&operation) else {
            return;
        };
        if cancel.is_cancelled() {
            return;
        }
        match result {
            Ok(prepared) => {
                let manifest = prepared.manifest();
                entry.status.preview = Some(SnapshotPreview {
                    format_version: manifest.version,
                    app_version: manifest.app_version.clone(),
                    created_at: manifest.created_at,
                    database_bytes: manifest.database_bytes,
                    database_sha256: manifest.database_sha256.clone(),
                    approval: Uuid::new_v4(),
                });
                entry.prepared = Some(prepared);
                entry.status.state = SnapshotState::AwaitingApproval;
            }
            Err(error) => fail(entry, error),
        }
    })
}

pub(super) fn restore(
    operations: Arc<SnapshotOperations>,
    operation: Uuid,
    epoch: Uuid,
    actor: UserContext,
    prepared: PreparedRestore,
) {
    tokio::spawn(async move {
        let result = operations.deployment.restore(epoch, actor, prepared).await;
        let mut entries = operations.entries();
        let Some(entry) = entries.get_mut(&operation) else {
            return;
        };
        match result {
            Ok(epoch) => {
                entry.status.epoch = epoch;
                entry.status.state = SnapshotState::Completed;
                entry.discard();
            }
            Err(error) => fail(entry, error.to_string()),
        }
    });
}

pub(super) fn fail(entry: &mut super::Entry, error: String) {
    entry.discard();
    entry.status.state = SnapshotState::Failed;
    entry.status.error = Some(error);
}

struct CancelRead {
    input: File,
    cancel: CancellationToken,
}
impl Read for CancelRead {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("snapshot verification cancelled"));
        }
        self.input.read(buffer)
    }
}
