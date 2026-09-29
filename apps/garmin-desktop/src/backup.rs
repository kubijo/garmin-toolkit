//! Native pickers hand paths to this adapter; the shared service receives only bounded bytes.

use eframe::egui;
use garmin_service_api::snapshots::{
    MAX_SNAPSHOT_CHUNK, SnapshotReply, SnapshotRequest, SnapshotState, SnapshotStatus,
};
use garmin_services::{
    UserContext,
    deployment::Deployment,
    snapshots::{SnapshotOperations, SnapshotSession},
};
use garmin_ui::backup::{SelectedFile, State};
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

pub enum Job {
    Backup(PathBuf),
    Restore(PathBuf),
}

pub enum Command {
    Approve(Uuid),
    Cancel,
}

pub struct Controller {
    pub state: State,
    pub file: Option<SelectedFile>,
    deployment: Arc<Deployment>,
    operations: Arc<SnapshotOperations>,
    events: mpsc::Receiver<State>,
    sender: mpsc::Sender<State>,
    commands: Option<UnboundedSender<Command>>,
    task: Option<JoinHandle<()>>,
    context: egui::Context,
    data_root: PathBuf,
}

impl Controller {
    pub fn new(
        deployment: Arc<Deployment>,
        context: egui::Context,
        data_root: PathBuf,
    ) -> Result<Self, String> {
        let operations = SnapshotOperations::new(
            Arc::clone(&deployment),
            garmin_storage::snapshot::Limits::default(),
        )
        .map_err(|error| error.message)?;
        let (sender, events) = mpsc::channel();
        Ok(Self {
            state: State::Idle,
            file: None,
            deployment,
            operations,
            events,
            sender,
            commands: None,
            task: None,
            context,
            data_root,
        })
    }

    pub fn start(&mut self, actor: UserContext, epoch: Uuid, job: Job) {
        if self.state.busy() {
            return;
        }
        self.file = match &job {
            Job::Backup(path) => path
                .file_name()
                .map(|name| SelectedFile::SaveDestination(name.to_string_lossy().into_owned())),
            Job::Restore(path) => path
                .file_name()
                .map(|name| SelectedFile::RestoreSource(name.to_string_lossy().into_owned())),
        };
        if let Job::Backup(path) = &job
            && let Err(error) = validate_destination(&self.data_root, path)
        {
            self.state = State::Failed(error);
            return;
        }
        self.join();
        self.state = State::Starting;
        let (commands, receiver) = unbounded_channel();
        self.commands = Some(commands);
        let operations = Arc::clone(&self.operations);
        let deployment = Arc::clone(&self.deployment);
        let sender = self.sender.clone();
        let context = self.context.clone();
        match std::thread::Builder::new()
            .name("garmin-snapshots".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
                    .and_then(|runtime| {
                        runtime.block_on(async {
                            if deployment.epoch() != epoch {
                                return Err("database changed; start again".into());
                            }
                            let session = operations
                                .connect(actor)
                                .await
                                .map_err(|error| error.message)?;
                            let mut transfer = Transfer {
                                session,
                                commands: receiver,
                                sender: &sender,
                                context: &context,
                                operation: None,
                            };
                            let result = transfer.run(job).await;
                            transfer.cleanup().await;
                            result
                        })
                    });
                let state = result.unwrap_or_else(State::Failed);
                let _ = sender.send(state);
                context.request_repaint();
            }) {
            Ok(task) => self.task = Some(task),
            Err(error) => {
                self.commands = None;
                self.state = State::Failed(error.to_string());
            }
        }
    }

    pub fn poll(&mut self) {
        for state in self.events.try_iter() {
            self.state = state;
        }
        if self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            self.join();
        }
    }

    pub fn cancel(&self) {
        self.send(Command::Cancel);
    }
    pub fn clear(&mut self) {
        if !self.state.busy() {
            self.join();
            self.state = State::Idle;
            self.file = None;
        }
    }
    pub fn approve(&mut self) {
        if let State::Running(status) = &mut self.state
            && let Some(preview) = status.preview.take()
        {
            let approval = preview.approval;
            // Freeze the controls as soon as approval is sent, before the next status event.
            status.state = SnapshotState::Restoring;
            self.send(Command::Approve(approval));
        }
    }
    fn send(&self, command: Command) {
        if let Some(sender) = &self.commands {
            let _ = sender.send(command);
        }
    }
    fn join(&mut self) {
        if let Some(task) = self.task.take()
            && task.join().is_err()
        {
            self.state = State::Failed("snapshot worker stopped unexpectedly".into());
        }
        self.commands = None;
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel();
        self.join();
    }
}

struct Transfer<'a> {
    session: SnapshotSession,
    commands: UnboundedReceiver<Command>,
    sender: &'a mpsc::Sender<State>,
    context: &'a egui::Context,
    operation: Option<Uuid>,
}

impl Transfer<'_> {
    async fn execute(&self, request: SnapshotRequest) -> Result<SnapshotReply, String> {
        self.session
            .execute(request)
            .await
            .map_err(|error| error.message)
    }
    fn report(&self, status: &SnapshotStatus) {
        let _ = self.sender.send(State::Running(status.clone()));
        self.context.request_repaint();
    }
    async fn run(&mut self, job: Job) -> Result<State, String> {
        match job {
            Job::Backup(path) => self.backup(path).await,
            Job::Restore(path) => self.restore(path).await,
        }
    }
    fn cancelled(&mut self) -> bool {
        matches!(
            self.commands.try_recv(),
            Ok(Command::Cancel) | Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        )
    }
    async fn backup(&mut self, path: PathBuf) -> Result<State, String> {
        let mut status = status(
            self.execute(SnapshotRequest::BeginBackup {
                operation: uuid::Uuid::new_v4(),
            })
            .await?,
        )?;
        self.operation = Some(status.operation);
        status = self.wait(status, SnapshotState::DownloadReady).await?;
        if status.state == SnapshotState::Cancelled {
            return Ok(State::Cancelled);
        }
        let parent = path.parent().ok_or("backup destination has no parent")?;
        let mut output =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        let mut offset = 0;
        loop {
            if self.cancelled() {
                return Ok(State::Cancelled);
            }
            let SnapshotReply::Chunk { bytes, end, .. } = self
                .execute(SnapshotRequest::Read {
                    operation: status.operation,
                    offset,
                    max_bytes: MAX_SNAPSHOT_CHUNK,
                })
                .await?
            else {
                return Err("expected backup bytes".into());
            };
            output
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
            offset += bytes.len() as u64;
            status.transferred = offset;
            self.report(&status);
            if end {
                break;
            }
        }
        if self.cancelled() {
            return Ok(State::Cancelled);
        }
        output
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        output.persist(&path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(State::Saved)
    }
    async fn restore(&mut self, path: PathBuf) -> Result<State, String> {
        let mut input = File::open(path).map_err(|error| error.to_string())?;
        let metadata = input.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("backup must be a regular file".into());
        }
        let mut current = status(
            self.execute(SnapshotRequest::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: metadata.len(),
            })
            .await?,
        )?;
        self.operation = Some(current.operation);
        let mut buffer = vec![0; MAX_SNAPSHOT_CHUNK as usize];
        loop {
            if self.cancelled() {
                return Ok(State::Cancelled);
            }
            let length = input.read(&mut buffer).map_err(|error| error.to_string())?;
            if length == 0 {
                break;
            }
            current = status(
                self.execute(SnapshotRequest::Upload {
                    operation: current.operation,
                    offset: current.transferred,
                    bytes: buffer[..length].to_vec(),
                })
                .await?,
            )?;
            self.report(&current);
        }
        current = status(
            self.execute(SnapshotRequest::Verify {
                operation: current.operation,
            })
            .await?,
        )?;
        current = self.wait(current, SnapshotState::AwaitingApproval).await?;
        if current.state == SnapshotState::Cancelled {
            return Ok(State::Cancelled);
        }
        let Some(Command::Approve(approval)) = self.commands.recv().await else {
            return Ok(State::Cancelled);
        };
        current = status(
            self.execute(SnapshotRequest::Approve {
                operation: current.operation,
                approval,
            })
            .await?,
        )?;
        current = self.wait(current, SnapshotState::Completed).await?;
        if current.state == SnapshotState::Completed {
            Ok(State::Restored)
        } else {
            Ok(State::Cancelled)
        }
    }
    async fn wait(
        &mut self,
        mut current: SnapshotStatus,
        target: SnapshotState,
    ) -> Result<SnapshotStatus, String> {
        loop {
            self.report(&current);
            if current.state == target || current.state == SnapshotState::Cancelled {
                return Ok(current);
            }
            if current.state == SnapshotState::Failed {
                return Err(current
                    .error
                    .unwrap_or_else(|| "snapshot operation failed".into()));
            }
            tokio::select! {
                command = self.commands.recv(), if current.state != SnapshotState::Restoring => {
                    if matches!(command, None | Some(Command::Cancel)) {
                        return status(self.execute(SnapshotRequest::Cancel { operation: current.operation }).await?);
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            current = status(
                self.execute(SnapshotRequest::Status {
                    operation: current.operation,
                })
                .await?,
            )?;
        }
    }
    async fn cleanup(&self) {
        if let Some(operation) = self.operation {
            let _ = self.execute(SnapshotRequest::Cancel { operation }).await;
            let _ = self.execute(SnapshotRequest::Release { operation }).await;
        }
    }
}
fn status(reply: SnapshotReply) -> Result<SnapshotStatus, String> {
    match reply {
        SnapshotReply::Status(status) => Ok(status),
        _ => Err("expected snapshot status".into()),
    }
}

fn validate_destination(root: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("backup destination has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if parent.starts_with(root) {
        return Err("Save the backup outside the application's data directory".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
