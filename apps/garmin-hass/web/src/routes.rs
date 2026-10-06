//! Browser adapter for profile-bound route sessions and bounded File slices.

use garmin_model::{artifact::ByteCount, identity::UserId};
use garmin_service_api::{
    ApplicationService as _,
    routes::{MAX_UPLOAD_CHUNK, RouteReply, RouteRequest, RouteService as _, RouteServiceClient},
};
use garmin_ui::routes::{Action, State as ViewState, Workspace};
use std::{cell::RefCell, rc::Rc, time::Duration};
use wasm_bindgen_futures::{JsFuture, spawn_local};

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
        let result = async {
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
                Action::Request(request) => call(&client, request).await.map(Some),
                Action::Import => upload(&client, &shared, token).await,
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
            }
        }
        .await;
        let mut state = shared.borrow_mut();
        if state.routes.token == token {
            match result {
                Ok(Some(reply)) => state.routes.view.accept(reply),
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
            call(
                client,
                RouteRequest::Append {
                    operation,
                    offset: u64::from(offset),
                    bytes,
                },
            )
            .await?;
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
