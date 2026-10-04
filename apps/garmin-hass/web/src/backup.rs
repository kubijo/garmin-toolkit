//! Browser snapshot adapter: bounded uploads, streamed downloads, reconnect, explicit approval.

use super::{ApplicationServiceClient, JsFuture, Rc, RefCell, State, UserId, spawn_local};
use garmin_service_api::files::{Operation, Selection};
use garmin_service_api::snapshots::{
    MAX_SNAPSHOT_CHUNK, SnapshotReply, SnapshotRequest, SnapshotServiceClient, SnapshotSource,
    SnapshotState, SnapshotStatus,
};
use garmin_service_api::{ApplicationService as _, snapshots::SnapshotService as _};
use garmin_ui::backup::{Action, RestoreSummary, SelectedFile, State as ViewState};
use uuid::Uuid;
use web_time::Instant;

pub(super) mod chooser;
mod recovery;

#[derive(Default)]
pub(super) struct Controller {
    pub state: ViewState,
    pub file: Option<SelectedFile>,
    command: Option<Command>,
    chooser: Option<chooser::Host>,
    recovery: recovery::Discovery,
}

enum Command {
    Cancel,
    Approve(Uuid),
}

impl Controller {
    pub fn action(&mut self, action: Action) {
        match action {
            Action::Clear if !self.state.busy() => *self = Self::default(),
            Action::Cancel if !self.state.switching() => self.command = Some(Command::Cancel),
            Action::Approve => {
                if let ViewState::Running(status) = &mut self.state
                    && let Some(preview) = status.preview.take()
                {
                    self.command = Some(Command::Approve(preview.approval));
                    status.state = SnapshotState::Restoring;
                }
            }
            _ => {}
        }
    }
}

pub(super) fn start(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    actor: UserId,
    action: Action,
    intl: &garmin_i18n::Intl,
) {
    if matches!(action, Action::ServerBackup | Action::ServerRestore) {
        let operation = if action == Action::ServerBackup {
            Operation::Save
        } else {
            Operation::Open
        };
        chooser::open(shared, context, actor, operation, intl);
        return;
    }
    {
        let mut state = shared.borrow_mut();
        if state.backup.state.busy() || state.backup.chooser.is_some() || state.client.is_none() {
            return;
        }
        state.backup.state = ViewState::Starting;
        state.backup.file = None;
        state.backup.command = None;
    }
    spawn_local(async move {
        let source = if action == Action::Restore {
            let Some(file) = rfd::AsyncFileDialog::new()
                .add_filter("Garmin backup", &["zst"])
                .pick_file()
                .await
            else {
                shared.borrow_mut().backup = Controller::default();
                context.request_repaint();
                return;
            };
            shared.borrow_mut().backup.file = Some(SelectedFile::RestoreSource(file.file_name()));
            Some(file.inner().clone())
        } else {
            shared.borrow_mut().backup.file = Some(SelectedFile::SaveDestination(
                "garmin-backup.tar.zst".into(),
            ));
            None
        };
        Runner {
            shared,
            context,
            actor,
            origin: if source.is_some() {
                SnapshotSource::Upload
            } else {
                SnapshotSource::Download
            },
            source,
            server: None,
            operation: None,
            recover: false,
            restore_facts: None,
            restore_started: None,
        }
        .run()
        .await;
    });
}

struct Runner {
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    actor: UserId,
    source: Option<web_sys::File>,
    server: Option<Selection>,
    operation: Option<Uuid>,
    origin: SnapshotSource,
    recover: bool,
    restore_facts: Option<(u64, i64)>,
    restore_started: Option<Instant>,
}

enum Failure {
    Disconnected,
    Domain(String),
}

impl Runner {
    async fn run(mut self) {
        loop {
            let app = self.shared.borrow().client.clone();
            let Some(app) = app else {
                super::wait_milliseconds(250).await;
                continue;
            };
            match self.connect(&app).await {
                Ok(session) => match self.transfer(&app, &session).await {
                    Ok(state) => {
                        if !matches!(state, ViewState::DownloadStarted) {
                            self.cleanup(&session).await;
                        }
                        self.publish(state);
                        return;
                    }
                    Err(Failure::Domain(error)) => {
                        self.cleanup(&session).await;
                        self.publish(ViewState::Failed(error));
                        return;
                    }
                    Err(Failure::Disconnected) => {}
                },
                Err(Failure::Domain(error)) => {
                    self.publish(ViewState::Failed(error));
                    return;
                }
                Err(Failure::Disconnected) => {}
            }
            // Never replay a confirmation after losing its response. Resume rotates approvals.
            {
                let mut state = self.shared.borrow_mut();
                if matches!(state.backup.command, Some(Command::Approve(_))) {
                    state.backup.command = None;
                }
            }
            super::wait_milliseconds(500).await;
        }
    }

    async fn connect(
        &self,
        app: &ApplicationServiceClient,
    ) -> Result<SnapshotServiceClient, Failure> {
        app.snapshots(self.actor, self.operation)
            .await
            .map_err(|_| Failure::Disconnected)?
            .map_err(Failure::Domain)
    }

    fn publish(&self, state: ViewState) {
        self.shared.borrow_mut().backup.state = state;
        self.context.request_repaint();
    }

    async fn transfer(
        &mut self,
        app: &ApplicationServiceClient,
        session: &SnapshotServiceClient,
    ) -> Result<ViewState, Failure> {
        let mut current = if let Some(operation) = self.operation {
            let request = if self.recover {
                SnapshotRequest::Recover { operation }
            } else {
                SnapshotRequest::Resume { operation }
            };
            let current = status(request_status(session, request).await?)?;
            self.recover = false;
            current
        } else {
            let operation = Uuid::new_v4();
            self.operation = Some(operation);
            if let Some(selection) = &self.server {
                app.snapshot_file(operation, selection.clone())
                    .await
                    .map_err(|_| Failure::Disconnected)?
                    .map_err(Failure::Domain)?
            } else if let Some(file) = &self.source {
                status(
                    request_status(
                        session,
                        SnapshotRequest::BeginRestore {
                            operation,
                            bytes: u64::from(file_size(file)?),
                        },
                    )
                    .await?,
                )?
            } else {
                status(request_status(session, SnapshotRequest::BeginBackup { operation }).await?)?
            }
        };
        self.operation = Some(current.operation);
        loop {
            let command = self.shared.borrow_mut().backup.command.take();
            if let Some(command) = command {
                if matches!(command, Command::Approve(_))
                    && current.state == SnapshotState::AwaitingApproval
                {
                    self.restore_started = Some(Instant::now());
                }
                current = self.command(session, &current, command).await?;
                continue;
            }
            if let Some(preview) = &current.preview {
                self.restore_facts = Some((preview.database_bytes, preview.created_at));
            }
            self.publish(ViewState::Running(current.clone()));
            match current.state {
                SnapshotState::Uploading if self.origin == SnapshotSource::Upload => {
                    current = self.upload(session, &current).await?;
                }
                SnapshotState::DownloadReady => {
                    let ticket = app
                        .snapshot_download(current.operation)
                        .await
                        .map_err(|_| Failure::Disconnected)?
                        .map_err(Failure::Domain)?;
                    super::trigger_download(&ticket).map_err(Failure::Domain)?;
                    return Ok(ViewState::DownloadStarted);
                }
                SnapshotState::Completed => {
                    return Ok(if matches!(self.origin, SnapshotSource::ServerSave(_)) {
                        ViewState::Saved
                    } else {
                        ViewState::Restored(RestoreSummary {
                            elapsed: self.restore_started.map(|started| started.elapsed()),
                            archive_bytes: current.total,
                            database_bytes: self.restore_facts.map(|(bytes, _)| bytes),
                            created_at: self.restore_facts.map(|(_, created_at)| created_at),
                        })
                    });
                }
                SnapshotState::Cancelled => return Ok(ViewState::Cancelled),
                SnapshotState::Failed => {
                    return Err(Failure::Domain(
                        current
                            .error
                            .unwrap_or_else(|| "backup operation failed".into()),
                    ));
                }
                _ => {
                    super::wait_milliseconds(100).await;
                    current = status(
                        request_status(
                            session,
                            SnapshotRequest::Status {
                                operation: current.operation,
                            },
                        )
                        .await?,
                    )?;
                }
            }
        }
    }

    async fn command(
        &self,
        session: &SnapshotServiceClient,
        current: &SnapshotStatus,
        command: Command,
    ) -> Result<SnapshotStatus, Failure> {
        let operation = current.operation;
        let request = match command {
            Command::Cancel if current.state != SnapshotState::Restoring => {
                SnapshotRequest::Cancel { operation }
            }
            Command::Approve(approval) if current.state == SnapshotState::AwaitingApproval => {
                SnapshotRequest::Approve {
                    operation,
                    approval,
                }
            }
            _ => SnapshotRequest::Status { operation },
        };
        status(request_status(session, request).await?)
    }

    async fn upload(
        &self,
        session: &SnapshotServiceClient,
        current: &SnapshotStatus,
    ) -> Result<SnapshotStatus, Failure> {
        let file = self.source.as_ref().ok_or_else(|| {
            Failure::Domain("select the backup file again to resume upload".into())
        })?;
        let total = u64::from(file_size(file)?);
        if current.total != Some(total) || current.transferred > total {
            return Err(Failure::Domain(
                "selected file does not match this upload".into(),
            ));
        }
        if current.transferred == total {
            return status(
                request_status(
                    session,
                    SnapshotRequest::Verify {
                        operation: current.operation,
                    },
                )
                .await?,
            );
        }
        let offset = u32::try_from(current.transferred)
            .map_err(|error| Failure::Domain(error.to_string()))?;
        let end = u64::from(offset)
            .saturating_add(u64::from(MAX_SNAPSHOT_CHUNK))
            .min(total);
        let end = u32::try_from(end).map_err(|error| Failure::Domain(error.to_string()))?;
        let blob = file
            .slice_with_f64_and_f64(f64::from(offset), f64::from(end))
            .map_err(|error| js_failure(&error))?;
        let buffer = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|error| js_failure(&error))?;
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        status(
            request_status(
                session,
                SnapshotRequest::Upload {
                    operation: current.operation,
                    offset: u64::from(offset),
                    bytes,
                },
            )
            .await?,
        )
    }

    async fn cleanup(&self, session: &SnapshotServiceClient) {
        // A failed recovery never acquired the operation: leave its owner untouched.
        if self.recover {
            return;
        }
        if let Some(operation) = self.operation {
            let _ = session.execute(SnapshotRequest::Cancel { operation }).await;
            let _ = session
                .execute(SnapshotRequest::Release { operation })
                .await;
        }
    }
}

async fn request_status(
    session: &SnapshotServiceClient,
    request: SnapshotRequest,
) -> Result<SnapshotReply, Failure> {
    session
        .execute(request)
        .await
        .map_err(|_| Failure::Disconnected)?
        .map_err(|error| Failure::Domain(error.message))
}

fn status(reply: SnapshotReply) -> Result<SnapshotStatus, Failure> {
    match reply {
        SnapshotReply::Status(status) => Ok(status),
        _ => Err(Failure::Domain("snapshot status was not returned".into())),
    }
}

fn file_size(file: &web_sys::File) -> Result<u32, Failure> {
    let size = file.size();
    if !size.is_finite() || size <= 0.0 || size > f64::from(u32::MAX) || size.fract() != 0.0 {
        return Err(Failure::Domain(
            "backup file is empty or exceeds the browser upload limit".into(),
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated positive integer within u32 above"
    )]
    Ok(size as u32)
}

fn js_failure(error: &wasm_bindgen::JsValue) -> Failure {
    Failure::Domain(super::js_reason(error))
}

pub(super) fn show(
    ui: &mut eframe::egui::Ui,
    intl: &garmin_i18n::Intl,
    shared: &Rc<RefCell<State>>,
    actor: Option<UserId>,
) -> Option<Action> {
    eframe::egui::ScrollArea::vertical()
        .id_salt("backup-page")
        .show(ui, |ui| {
            if let Some(actor) = actor {
                let inline_padding = if ui.available_width() < 480.0 { 16 } else { 24 };
                eframe::egui::Frame::NONE
                    .inner_margin(eframe::egui::Margin::symmetric(inline_padding, 0))
                    .show(ui, |ui| recovery::show(ui, intl, shared, actor));
            }
            let state = shared.borrow();
            garmin_ui::backup::show(
                ui,
                &garmin_ui::backup::Props {
                    server_files: true,
                    intl,
                    state: &state.backup.state,
                    file: state.backup.file.as_ref(),
                    enabled: actor.is_some()
                        && state.client.is_some()
                        && !state.backup.recovery.loading,
                },
            )
        })
        .inner
}

impl super::App {
    pub(super) fn update_server_chooser(&self) {
        chooser::update(
            &self.intl,
            &self.shared,
            &self.context,
            self.selected_profile
                .and_then(|index| self.profiles.get(index))
                .map(|profile| profile.user.id()),
        );
    }

    pub(super) fn show_backup_outcome(&self, ui: &mut eframe::egui::Ui) -> Option<Action> {
        let state = self.shared.borrow();
        garmin_ui::backup::show_operation(
            ui,
            &garmin_ui::backup::Props {
                intl: &self.intl,
                state: &state.backup.state,
                file: state.backup.file.as_ref(),
                enabled: false,
                server_files: true,
            },
        )
    }
    pub(super) fn reset_changed_database(&mut self) {
        let epoch = self.shared.borrow().epoch.clone();
        if self.epoch == epoch {
            return;
        }
        self.epoch = epoch;
        {
            let mut state = self.shared.borrow_mut();
            chooser::close(&mut state.backup, &self.context);
            state.backup.recovery = recovery::Discovery::default();
        }
        let pending_fit_import = self.shared.borrow().pending_fit_import.clone();
        self.selected_profile = None;
        self.sync_file_window_owner();
        self.shared.borrow_mut().pending_fit_import = pending_fit_import;
        self.page = garmin_ui::workspace::Page::Activities;
        self.profile_menu_expanded = false;
        self.selected_activity = 0;
        self.create_profile = None;
        self.profiles = Rc::new(Vec::new());
        self.profile_presentations.clear();
        self.activity_presentations.clear();
        self.activity_detail = None;
        self.activity_workspace = garmin_ui::activity::Workspace::new(&self.map_runtime);
        self.applied_preferences = None;
        self.avatar_editor = None;
        self.device_browser = None;
        self.device_fit_preview = None;
        self.device_fit_plan = None;
        self.device_fit_job = None;
    }
}
