//! Browser adapter for profile-bound route sessions and bounded File slices.

use garmin_model::{
    artifact::{AcquisitionOperationId, ByteCount},
    identity::UserId,
};
use garmin_service_api::{
    ApplicationService as _, ApplicationServiceClient, DeviceSnapshot,
    course_transfer::{
        CourseCleanupReview, CourseTarget, CourseTransferPreparation, CourseTransferService as _,
        CourseTransferServiceClient, CourseTransferStatus,
    },
    routes::{MAX_UPLOAD_CHUNK, RouteReply, RouteRequest, RouteService as _, RouteServiceClient},
};
use garmin_ui::routes::{Action, State as ViewState, Workspace};
use std::{cell::RefCell, rc::Rc, time::Duration};
use wasm_bindgen_futures::{JsFuture, spawn_local};

enum Completion {
    Route(RouteReply),
    Targets(String, Vec<CourseTarget>, Vec<CourseTransferStatus>),
    Prepared(CourseTransferPreparation),
    Status(CourseTransferStatus),
    Cleanup(CourseCleanupReview),
}

#[derive(Default)]
pub(super) struct Controller {
    view: ViewState,
    scope: Option<(UserId, Option<String>)>,
    token: uuid::Uuid,
    client: Option<RouteServiceClient>,
    polled: Option<web_time::Instant>,
}

impl Controller {
    pub fn invalidate(&mut self) {
        *self = Self {
            token: uuid::Uuid::new_v4(),
            ..Self::default()
        };
    }
    pub fn disconnect(&mut self) {
        self.client = None;
        self.token = uuid::Uuid::new_v4();
        if self.view.busy {
            self.view
                .fail("connection interrupted; retry the route operation".into());
        }
    }
}

pub(super) fn show(
    ui: &mut eframe::egui::Ui,
    intl: &garmin_i18n::Intl,
    shared: &Rc<RefCell<super::State>>,
    workspace: &mut Workspace,
    actor: UserId,
    devices: &[DeviceSnapshot],
) {
    let action = {
        let mut state = shared.borrow_mut();
        let scope = (actor, state.epoch.clone());
        if state.routes.scope != Some(scope.clone()) {
            state.routes = Controller {
                scope: Some(scope),
                token: uuid::Uuid::new_v4(),
                ..Controller::default()
            };
            state.routes.view.queue(RouteRequest::List { offset: 0 });
        }
        state.routes.view.devices = devices.to_vec();
        workspace.show(ui, intl, &mut state.routes.view)
    };
    if let Some(action) = action {
        match action {
            Action::Request(request) => shared.borrow_mut().routes.view.queue(request),
            action => start(shared, ui.ctx(), actor, action),
        }
    }
    let request = {
        let mut state = shared.borrow_mut();
        let routes = &mut state.routes;
        if routes
            .polled
            .is_none_or(|time| time.elapsed() >= Duration::from_millis(100))
        {
            let request = routes.view.take_request();
            if request.is_some() {
                routes.polled = Some(web_time::Instant::now());
            }
            request
        } else {
            None
        }
    };
    if let Some(request) = request {
        start(shared, ui.ctx(), actor, Action::Request(request));
    }
    let state = shared.borrow();
    if state.routes.view.busy || state.routes.view.pending.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

fn start(
    shared: &Rc<RefCell<super::State>>,
    context: &eframe::egui::Context,
    actor: UserId,
    action: Action,
) {
    let (application, client, token) = {
        let mut state = shared.borrow_mut();
        let Some(application) = state.client.clone() else {
            state.routes.view.fail("connection unavailable".into());
            return;
        };
        state.routes.view.busy = true;
        (application, state.routes.client.clone(), state.routes.token)
    };
    let shared = Rc::clone(shared);
    let context = context.clone();
    spawn_local(async move {
        let result = execute(
            &application,
            client,
            &shared,
            actor,
            action,
            token,
            &context,
        )
        .await;
        let mut state = shared.borrow_mut();
        if state.routes.token == token {
            match result {
                Ok(Some(Completion::Route(reply))) => state.routes.view.accept(reply),
                Ok(Some(Completion::Targets(key, targets, receipts))) => {
                    state.routes.view.transfer_targets(key, targets, receipts);
                }
                Ok(Some(Completion::Prepared(result))) => {
                    state.routes.view.transfer_prepared(result);
                }
                Ok(Some(Completion::Status(status))) => state.routes.view.transfer_status(status),
                Ok(Some(Completion::Cleanup(review))) => state.routes.view.transfer_cleanup(review),
                Ok(None) => state.routes.view.busy = false,
                Err(error) => {
                    state.routes.client = None;
                    state.routes.view.fail(error);
                }
            }
        }
        context.request_repaint();
    });
}

async fn execute(
    application: &ApplicationServiceClient,
    client: Option<RouteServiceClient>,
    shared: &Rc<RefCell<super::State>>,
    actor: UserId,
    action: Action,
    token: uuid::Uuid,
    context: &eframe::egui::Context,
) -> Result<Option<Completion>, String> {
    let client = match client {
        Some(client) => client,
        None => application
            .routes(actor)
            .await
            .map_err(|error| error.to_string())??,
    };
    if shared.borrow().routes.token != token {
        return Ok(None);
    }
    shared.borrow_mut().routes.client = Some(client.clone());
    match action {
        Action::Request(request) => call(&client, request)
            .await
            .map(Completion::Route)
            .map(Some),
        Action::Import { replace } => upload(&client, shared, token, replace, context)
            .await
            .map(|reply| reply.map(Completion::Route)),
        Action::Download(artifact) => {
            let ticket = application
                .route_download(actor, artifact)
                .await
                .map_err(|error| error.to_string())??;
            if shared.borrow().routes.token == token {
                super::trigger_download(&ticket)?;
            }
            Ok(None)
        }
        other => execute_transfer(application, actor, other).await,
    }
}

async fn execute_transfer(
    application: &ApplicationServiceClient,
    actor: UserId,
    action: Action,
) -> Result<Option<Completion>, String> {
    let device_key = match &action {
        Action::ChooseTransferDevice { device_key, .. }
        | Action::PrepareTransfer { device_key, .. }
        | Action::ApproveTransfer { device_key, .. }
        | Action::PollTransfer { device_key, .. }
        | Action::CancelTransfer { device_key, .. }
        | Action::AcceptTransfer { device_key, .. }
        | Action::PrepareTransferCleanup { device_key, .. }
        | Action::ApproveTransferCleanup { device_key, .. } => device_key,
        Action::BeginTransfer(_) | Action::DismissTransfer => return Ok(None),
        Action::Request(_) | Action::Import { .. } | Action::Download(_) => {
            return Err("unexpected route action".into());
        }
    };
    let service = transfer_client(application, actor, device_key).await?;
    match action {
        Action::ChooseTransferDevice {
            generation,
            device_key,
        } => {
            let targets = service
                .targets(generation)
                .await
                .map_err(|error| error.to_string())??;
            let receipts = service
                .list(generation)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Targets(device_key, targets, receipts)))
        }
        Action::PrepareTransfer {
            generation,
            storage_id,
            ..
        } => {
            let result = service
                .prepare(generation, storage_id)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Prepared(result)))
        }
        Action::ApproveTransfer { approval, .. } => {
            let status = service
                .approve(approval)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Status(status)))
        }
        Action::PollTransfer { transfer, .. } => {
            let status = service
                .status(transfer)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Status(status)))
        }
        Action::CancelTransfer { transfer, .. } => {
            service
                .cancel(transfer)
                .await
                .map_err(|error| error.to_string())??;
            Ok(None)
        }
        Action::AcceptTransfer { transfer, .. } => {
            let status = service
                .accept(transfer)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Status(status)))
        }
        Action::PrepareTransferCleanup { transfer, .. } => {
            let review = service
                .prepare_cleanup(transfer)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Cleanup(review)))
        }
        Action::ApproveTransferCleanup { approval, .. } => {
            let status = service
                .approve_cleanup(approval)
                .await
                .map_err(|error| error.to_string())??;
            Ok(Some(Completion::Status(status)))
        }
        Action::BeginTransfer(_)
        | Action::DismissTransfer
        | Action::Request(_)
        | Action::Import { .. }
        | Action::Download(_) => Err("unexpected transfer action".into()),
    }
}

async fn transfer_client(
    application: &ApplicationServiceClient,
    actor: UserId,
    device_key: &str,
) -> Result<CourseTransferServiceClient, String> {
    application
        .course_transfers(actor, device_key.to_owned())
        .await
        .map_err(|error| error.to_string())?
}

async fn call(client: &RouteServiceClient, request: RouteRequest) -> Result<RouteReply, String> {
    client
        .execute(request)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.message)
}

async fn upload(
    client: &RouteServiceClient,
    shared: &Rc<RefCell<super::State>>,
    token: uuid::Uuid,
    replace: Option<AcquisitionOperationId>,
    context: &eframe::egui::Context,
) -> Result<Option<RouteReply>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("GPX", &["gpx"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    if shared.borrow().routes.token != token {
        return Ok(None);
    }
    let file = file.inner();
    let size = file.size();
    if !size.is_finite() || size < 1.0 || size > 16.0 * 1024.0 * 1024.0 {
        return Err("GPX files must be between 1 byte and 16 MiB".into());
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "File.size is a nonnegative integer, checked above against the 16 MiB cap"
    )]
    let size = size as u32;
    if let Some(operation) = replace {
        let _ = call(client, RouteRequest::Cancel { operation }).await;
    }
    let RouteReply::Upload(upload) = call(
        client,
        RouteRequest::StartUpload {
            file_name: file.name(),
            size: ByteCount::from_u64(u64::from(size)),
        },
    )
    .await?
    else {
        return Err("unexpected upload response".into());
    };
    let operation = upload.operation;
    if shared.borrow().routes.token != token {
        let _ = call(client, RouteRequest::Cancel { operation }).await;
        return Ok(None);
    }
    shared.borrow_mut().routes.view.begin_upload(upload);
    context.request_repaint();
    let result = async {
        let mut offset = 0;
        while offset < size {
            if shared.borrow().routes.token != token {
                return Err("profile changed during GPX upload".into());
            }
            let end = offset
                .saturating_add(u32::try_from(MAX_UPLOAD_CHUNK).map_err(|error| error.to_string())?)
                .min(size);
            let slice = file
                .slice_with_f64_and_f64(f64::from(offset), f64::from(end))
                .map_err(|error| format!("{error:?}"))?;
            let buffer = JsFuture::from(slice.array_buffer())
                .await
                .map_err(|error| format!("{error:?}"))?;
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            let RouteReply::Upload(progress) = call(
                client,
                RouteRequest::Append {
                    operation,
                    offset: u64::from(offset),
                    bytes,
                },
            )
            .await?
            else {
                return Err("unexpected upload response".into());
            };
            if shared.borrow().routes.token == token {
                shared.borrow_mut().routes.view.begin_upload(progress);
                context.request_repaint();
            }
            offset = end;
        }
        call(client, RouteRequest::Inspect { operation })
            .await
            .map(Some)
    }
    .await;
    if result.is_err() {
        let _ = call(client, RouteRequest::Cancel { operation }).await;
    }
    result
}
