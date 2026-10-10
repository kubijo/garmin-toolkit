//! Desktop route adapter. Pickers and filesystem publication stay outside the shared UI/service.

use eframe::egui;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactId, ByteCount},
    identity::UserId,
};
use garmin_service_api::routes::{GpxUpload, MAX_UPLOAD_CHUNK, RouteReply, RouteRequest};
use garmin_service_api::{
    DeviceSnapshot,
    course_transfer::{
        CourseCleanupReview, CourseTarget, CourseTransferPhase, CourseTransferPreparation,
        CourseTransferStatus,
    },
};
use garmin_services::{
    UserContext,
    course_transfer::CourseTransfers,
    deployment::Deployment,
    maps::{MutationLocks, device::Connector},
    routes::operations::{RouteOperations, RouteSession},
};
use garmin_ui::routes::{Action, State, Workspace};
use std::{
    io::Write as _,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

enum Event {
    Reply(RouteReply),
    UploadStarted(GpxUpload),
    TransferTargets(String, Vec<CourseTarget>, Vec<CourseTransferStatus>),
    TransferPrepared(CourseTransferPreparation),
    TransferStatus(CourseTransferStatus),
    CleanupReview(CourseCleanupReview),
    Failed(String),
    Finished,
}

struct ActionContext {
    operations: Arc<RouteOperations>,
    transfers: Arc<CourseTransfers>,
    connector: Option<Arc<dyn Connector>>,
    deployment: Arc<Deployment>,
    epoch: Uuid,
    actor: UserId,
}

pub struct Controller {
    runtime: tokio::runtime::Runtime,
    operations: Result<Arc<RouteOperations>, String>,
    deployment: Arc<Deployment>,
    transfers: Arc<CourseTransfers>,
    scope: Option<(UserId, Uuid)>,
    state: State,
    workspace: Workspace,
    events: mpsc::Receiver<(Uuid, Event)>,
    sender: mpsc::Sender<(Uuid, Event)>,
    token: Uuid,
    task: Option<tokio::task::JoinHandle<()>>,
    transfer_polled: Option<Instant>,
}

impl Controller {
    pub fn invalidate(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.token = Uuid::new_v4();
        self.scope = None;
        self.state = State::default();
        self.transfer_polled = None;
    }

    pub fn busy(&self) -> bool {
        self.state.busy
    }

    pub fn new(
        deployment: Arc<Deployment>,
        map: &garmin_ui::activity::map_runtime::MapRuntimeHandle,
        mutations: Arc<MutationLocks>,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let operations = {
            let _entered = runtime.enter();
            RouteOperations::beside_host(Arc::clone(&deployment)).map_err(|error| error.message)
        };
        let (sender, events) = mpsc::channel();
        let transfers = CourseTransfers::new(Arc::clone(&deployment), mutations);
        Ok(Self {
            runtime,
            operations,
            deployment,
            transfers,
            scope: None,
            state: State::default(),
            workspace: Workspace::new(map),
            events,
            sender,
            token: Uuid::new_v4(),
            task: None,
            transfer_polled: None,
        })
    }

    pub fn poll(&mut self) {
        for (token, event) in self.events.try_iter() {
            if token != self.token {
                continue;
            }
            match event {
                Event::Reply(reply) => self.state.accept(reply),
                Event::UploadStarted(upload) => self.state.begin_upload(upload),
                Event::TransferTargets(key, targets, receipts) => {
                    self.state.transfer_targets(key, targets, receipts);
                }
                Event::TransferPrepared(result) => self.state.transfer_prepared(result),
                Event::TransferStatus(status) => self.state.transfer_status(status),
                Event::CleanupReview(review) => self.state.transfer_cleanup(review),
                Event::Failed(error) => self.state.fail(error),
                Event::Finished => self.state.busy = false,
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        intl: &garmin_i18n::Intl,
        actor: UserId,
        devices: &[(DeviceSnapshot, Arc<dyn Connector>)],
    ) {
        let scope = (actor, self.deployment.epoch());
        if self.scope != Some(scope) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
            self.scope = Some(scope);
            self.token = Uuid::new_v4();
            self.state = State::default();
            self.transfer_polled = None;
            self.state.queue(RouteRequest::List { offset: 0 });
        }
        self.poll();
        self.state.devices = devices
            .iter()
            .map(|(snapshot, _)| snapshot.clone())
            .collect();
        if let Some(action) = self.workspace.show(ui, intl, &mut self.state) {
            match action {
                Action::Request(request) => self.state.queue(request),
                action => self.start(ui.ctx(), actor, action, devices),
            }
        }
        if let Some(request) = self.state.take_request() {
            self.start(ui.ctx(), actor, Action::Request(request), devices);
        }
        let running = self.state.transfer.as_ref().and_then(|transfer| {
            let status = transfer.status.as_ref()?;
            if matches!(status.phase, CourseTransferPhase::Running) {
                Some((transfer.device_key.clone()?, status.transfer))
            } else {
                None
            }
        });
        if let Some((device_key, transfer)) = running {
            ui.ctx().request_repaint_after(Duration::from_secs(2));
            if !self.state.busy
                && self
                    .transfer_polled
                    .is_none_or(|time| time.elapsed() >= Duration::from_secs(2))
            {
                self.transfer_polled = Some(Instant::now());
                self.start(
                    ui.ctx(),
                    actor,
                    Action::PollTransfer {
                        device_key,
                        transfer,
                    },
                    devices,
                );
            }
        } else {
            self.transfer_polled = None;
        }
        if self.state.busy || self.state.pending.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn start(
        &mut self,
        context: &egui::Context,
        actor: UserId,
        action: Action,
        devices: &[(DeviceSnapshot, Arc<dyn Connector>)],
    ) {
        let operations = match &self.operations {
            Ok(value) => Arc::clone(value),
            Err(error) => {
                self.state.fail(error.clone());
                return;
            }
        };
        self.state.busy = true;
        let epoch = self.deployment.epoch();
        let deployment = Arc::clone(&self.deployment);
        let transfers = Arc::clone(&self.transfers);
        let selected = selected_device_key(&action);
        let connector = selected.and_then(|key| {
            devices
                .iter()
                .find(|(device, _)| device.key == key)
                .map(|(_, connector)| Arc::clone(connector))
        });
        let context = context.clone();
        let sender = self.sender.clone();
        let token = self.token;
        self.task = Some(self.runtime.spawn(async move {
            let report_upload = |upload| {
                let _ = sender.send((token, Event::UploadStarted(upload)));
                context.request_repaint();
            };
            let result = execute_action(
                ActionContext {
                    operations,
                    transfers,
                    connector,
                    deployment,
                    epoch,
                    actor,
                },
                action,
                &report_upload,
            )
            .await;
            let _ = sender.send((token, result.unwrap_or_else(Event::Failed)));
            context.request_repaint();
        }));
    }
}

fn selected_device_key(action: &Action) -> Option<&str> {
    match action {
        Action::ChooseTransferDevice { device_key, .. }
        | Action::PrepareTransfer { device_key, .. }
        | Action::ApproveTransfer { device_key, .. }
        | Action::PollTransfer { device_key, .. }
        | Action::CancelTransfer { device_key, .. }
        | Action::PrepareTransferCleanup { device_key, .. }
        | Action::ApproveTransferCleanup { device_key, .. } => Some(device_key),
        _ => None,
    }
}

async fn execute_action(
    context: ActionContext,
    action: Action,
    report_upload: &(dyn Fn(GpxUpload) + Send + Sync),
) -> Result<Event, String> {
    let ActionContext {
        operations,
        transfers,
        connector,
        deployment,
        epoch,
        actor,
    } = context;
    if epoch != deployment.epoch() {
        return Err("database changed; reopen routes".into());
    }
    let session = operations
        .connect_at(UserContext::new(actor), epoch)
        .await
        .map_err(|error| error.message)?;
    match action {
        Action::Request(request) => {
            if matches!(request, RouteRequest::UploadStatus { .. }) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            session
                .request(request)
                .await
                .map(Event::Reply)
                .map_err(|error| error.message)
        }
        Action::Import { replace } => upload(&session, replace, report_upload).await,
        Action::Download(artifact) => download(&session, artifact).await,
        other => execute_transfer(transfers, connector, actor, other).await,
    }
}

async fn execute_transfer(
    transfers: Arc<CourseTransfers>,
    connector: Option<Arc<dyn Connector>>,
    actor: UserId,
    action: Action,
) -> Result<Event, String> {
    let user = UserContext::new(actor);
    match action {
        Action::ChooseTransferDevice {
            generation,
            device_key,
        } => {
            let connector = connector.ok_or("selected device is disconnected")?;
            let targets = transfers
                .targets(user, generation, &device_key, connector.as_ref())
                .await
                .map_err(|error| format!("{error:#}"))?;
            let receipts = transfers
                .list(user, generation, &device_key)
                .await
                .map_err(|error| format!("{error:#}"))?;
            Ok(Event::TransferTargets(device_key, targets, receipts))
        }
        Action::PrepareTransfer {
            generation,
            device_key,
            storage_id,
        } => {
            let connector = connector.ok_or("selected device is disconnected")?;
            transfers
                .prepare(user, generation, device_key, &storage_id, connector)
                .await
                .map(Event::TransferPrepared)
                .map_err(|error| format!("{error:#}"))
        }
        Action::ApproveTransfer { approval, .. } => transfers
            .approve(user, approval)
            .await
            .map(Event::TransferStatus)
            .map_err(|error| format!("{error:#}")),
        Action::PollTransfer { transfer, .. } => {
            let connector = connector.ok_or("selected device is disconnected")?;
            transfers
                .status(user, transfer, connector.as_ref())
                .await
                .map(Event::TransferStatus)
                .map_err(|error| format!("{error:#}"))
        }
        Action::CancelTransfer { transfer, .. } => {
            transfers
                .cancel(user, transfer)
                .map_err(|error| format!("{error:#}"))?;
            Ok(Event::Finished)
        }
        Action::PrepareTransferCleanup { transfer, .. } => {
            let connector = connector.ok_or("selected device is disconnected")?;
            transfers
                .prepare_cleanup(user, transfer, connector.as_ref())
                .await
                .map(Event::CleanupReview)
                .map_err(|error| format!("{error:#}"))
        }
        Action::ApproveTransferCleanup { approval, .. } => {
            let connector = connector.ok_or("selected device is disconnected")?;
            transfers
                .approve_cleanup(user, approval, connector.as_ref())
                .await
                .map(Event::TransferStatus)
                .map_err(|error| format!("{error:#}"))
        }
        Action::BeginTransfer(_)
        | Action::DismissTransfer
        | Action::Request(_)
        | Action::Import { .. }
        | Action::Download(_) => Ok(Event::Finished),
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn upload(
    session: &RouteSession,
    replace: Option<AcquisitionOperationId>,
    report_upload: &(dyn Fn(GpxUpload) + Send + Sync),
) -> Result<Event, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("GPX", &["gpx"])
        .pick_file()
        .await
    else {
        return Ok(Event::Finished);
    };
    let mut input = tokio::fs::File::open(file.path())
        .await
        .map_err(|error| error.to_string())?;
    let size = input
        .metadata()
        .await
        .map_err(|error| error.to_string())?
        .len();
    if let Some(operation) = replace {
        let _ = session.request(RouteRequest::Cancel { operation }).await;
    }
    let RouteReply::Upload(upload) = session
        .request(RouteRequest::StartUpload {
            file_name: file.file_name(),
            size: ByteCount::from_u64(size),
        })
        .await
        .map_err(|error| error.message)?
    else {
        return Err("unexpected upload response".into());
    };
    let operation = upload.operation;
    report_upload(upload);
    let result = async {
        let mut offset = 0;
        loop {
            let mut bytes = vec![0; MAX_UPLOAD_CHUNK];
            let read = input
                .read(&mut bytes)
                .await
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            bytes.truncate(read);
            let RouteReply::Upload(progress) = session
                .request(RouteRequest::Append {
                    operation,
                    offset,
                    bytes,
                })
                .await
                .map_err(|error| error.message)?
            else {
                return Err("unexpected upload response".into());
            };
            report_upload(progress);
            offset += read as u64;
        }
        session
            .request(RouteRequest::Inspect { operation })
            .await
            .map(Event::Reply)
            .map_err(|error| error.message)
    }
    .await;
    if result.is_err() {
        let _ = session.request(RouteRequest::Cancel { operation }).await;
    }
    result
}

async fn download(session: &RouteSession, artifact: ArtifactId) -> Result<Event, String> {
    let prepared = session
        .download(artifact)
        .await
        .map_err(|error| error.message)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_file_name(prepared.file_name())
        .save_file()
        .await
    else {
        return Ok(Event::Finished);
    };
    let destination = file.path().to_owned();
    let mut delivery = prepared.begin().await.map_err(|error| error.message)?;
    tokio::task::spawn_blocking(move || {
        let parent = destination
            .parent()
            .ok_or("save destination has no parent")?;
        let mut stage =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        while let Some(chunk) = delivery.next_chunk() {
            stage.write_all(chunk).map_err(|error| error.to_string())?;
        }
        stage
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        stage
            .persist_noclobber(destination)
            .map_err(|error| error.to_string())?;
        Ok(Event::Finished)
    })
    .await
    .map_err(|error| error.to_string())?
}
