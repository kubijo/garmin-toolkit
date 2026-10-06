//! Desktop route adapter. Pickers and filesystem publication stay outside the shared UI/service.

use eframe::egui;
use garmin_model::{
    artifact::{ArtifactId, ByteCount},
    identity::UserId,
};
use garmin_service_api::routes::{MAX_UPLOAD_CHUNK, RouteReply, RouteRequest};
use garmin_services::{
    UserContext,
    deployment::Deployment,
    routes::operations::{RouteOperations, RouteSession},
};
use garmin_ui::routes::{Action, State, Workspace};
use std::{
    io::Write as _,
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

enum Event {
    Reply(RouteReply),
    Failed(String),
    Finished,
}

pub struct Controller {
    runtime: tokio::runtime::Runtime,
    operations: Result<Arc<RouteOperations>, String>,
    deployment: Arc<Deployment>,
    scope: Option<(UserId, Uuid)>,
    state: State,
    workspace: Workspace,
    events: mpsc::Receiver<(Uuid, Event)>,
    sender: mpsc::Sender<(Uuid, Event)>,
    token: Uuid,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Controller {
    pub fn invalidate(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.token = Uuid::new_v4();
        self.scope = None;
        self.state = State::default();
    }

    pub fn busy(&self) -> bool {
        self.state.busy
    }

    pub fn new(
        deployment: Arc<Deployment>,
        map: &garmin_ui::activity::map_runtime::MapRuntimeHandle,
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
        Ok(Self {
            runtime,
            operations,
            deployment,
            scope: None,
            state: State::default(),
            workspace: Workspace::new(map),
            events,
            sender,
            token: Uuid::new_v4(),
            task: None,
        })
    }

    pub fn poll(&mut self) {
        for (token, event) in self.events.try_iter() {
            if token != self.token {
                continue;
            }
            match event {
                Event::Reply(reply) => self.state.accept(reply),
                Event::Failed(error) => self.state.fail(error),
                Event::Finished => self.state.busy = false,
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, intl: &garmin_i18n::Intl, actor: UserId) {
        let scope = (actor, self.deployment.epoch());
        if self.scope != Some(scope) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
            self.scope = Some(scope);
            self.token = Uuid::new_v4();
            self.state = State::default();
            self.state.queue(RouteRequest::List { offset: 0 });
        }
        self.poll();
        if let Some(action) = self.workspace.show(ui, intl, &mut self.state) {
            match action {
                Action::Request(request) => self.state.queue(request),
                action => self.start(ui.ctx(), actor, action),
            }
        }
        if let Some(request) = self.state.take_request() {
            self.start(ui.ctx(), actor, Action::Request(request));
        }
        if self.state.busy || self.state.pending.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn start(&mut self, context: &egui::Context, actor: UserId, action: Action) {
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
        let context = context.clone();
        let sender = self.sender.clone();
        let token = self.token;
        self.task = Some(self.runtime.spawn(async move {
            let result = async {
                if epoch != deployment.epoch() {
                    return Err("database changed; reopen routes".to_owned());
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
                        Ok(Event::Reply(
                            session
                                .request(request)
                                .await
                                .map_err(|error| error.message)?,
                        ))
                    }
                    Action::Import => upload(&session).await,
                    Action::Download(artifact) => download(&session, artifact).await,
                }
            }
            .await;
            let _ = sender.send((token, result.unwrap_or_else(Event::Failed)));
            context.request_repaint();
        }));
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn upload(session: &RouteSession) -> Result<Event, String> {
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
            session
                .request(RouteRequest::Append {
                    operation,
                    offset,
                    bytes,
                })
                .await
                .map_err(|error| error.message)?;
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
