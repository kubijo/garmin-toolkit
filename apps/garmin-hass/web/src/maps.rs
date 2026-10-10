//! Thin RPC presentation connection. A browser disconnect never cancels the host job.

use eframe::egui;
use garmin_service_api::{
    ApplicationService as _,
    maps::{Command, MapService as _, MapServiceClient, Request, State as Snapshot},
};
use garmin_ui::maps::ClientError;
use std::{cell::RefCell, rc::Rc};
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;

#[derive(Default)]
pub(super) struct Controller {
    device: Option<String>,
    generation: Uuid,
    client: Option<MapServiceClient>,
    snapshot: Option<Snapshot>,
    pending: Option<Request>,
    error: Option<ClientError>,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    intl: &garmin_i18n::Intl,
    shared: &Rc<RefCell<super::State>>,
    device: &str,
) -> Option<(Uuid, Command)> {
    let state = shared.borrow();
    let maps = &state.maps;
    if let Some(error) = &maps.error {
        error.show(ui, intl);
    }
    if maps.device.as_deref() != Some(device) {
        ui.spinner();
        return None;
    }
    let Some(snapshot) = &maps.snapshot else {
        ui.spinner();
        return None;
    };
    ui.add_enabled_ui(
        maps.client.is_some() && maps.pending.is_none() && state.client.is_some(),
        |ui| garmin_ui::maps::show(ui, intl, snapshot).map(|command| (snapshot.revision, command)),
    )
    .inner
}

pub(super) fn open(shared: Rc<RefCell<super::State>>, context: egui::Context, device: String) {
    let (client, generation) = {
        let mut state = shared.borrow_mut();
        let Some(client) = state.client.clone() else {
            return;
        };
        if state.maps.device.as_ref() != Some(&device) {
            state.maps = Controller::default();
        }
        state.maps.device = Some(device.clone());
        state.maps.client = None;
        state.maps.generation = Uuid::new_v4();
        (client, state.maps.generation)
    };
    spawn_local(async move {
        let result = async {
            let client = client
                .maps(device)
                .await
                .map_err(|error| error.to_string())??;
            let watch = client.watch().await.map_err(|error| error.to_string())?;
            Ok::<_, String>((client, watch))
        }
        .await;
        let (client, mut watch) = match result {
            Ok(connection) => connection,
            Err(error) => {
                fail(&shared, &context, generation, error);
                context.request_repaint();
                return;
            }
        };
        {
            let mut state = shared.borrow_mut();
            if state.maps.generation != generation {
                return;
            }
            state.maps.client = Some(client);
            state.maps.error = None;
        }
        retry(Rc::clone(&shared), context.clone());
        loop {
            let snapshot = watch.borrow_and_update().map(|snapshot| snapshot.clone());
            match snapshot {
                Ok(snapshot) => {
                    let mut state = shared.borrow_mut();
                    if state.maps.generation != generation {
                        return;
                    }
                    state.maps.snapshot = Some(snapshot);
                }
                Err(error) => {
                    fail(&shared, &context, generation, error.to_string());
                    return;
                }
            }
            context.request_repaint();
            if let Err(error) = watch.changed().await {
                fail(&shared, &context, generation, error.to_string());
                context.request_repaint();
                return;
            }
        }
    });
}

pub(super) fn reconnect(shared: Rc<RefCell<super::State>>, context: egui::Context) {
    let device = shared.borrow().maps.device.clone();
    if let Some(device) = device {
        open(shared, context, device);
    }
}

pub(super) fn submit(
    shared: Rc<RefCell<super::State>>,
    context: egui::Context,
    revision: Uuid,
    command: Command,
) {
    {
        let mut state = shared.borrow_mut();
        if state.maps.pending.is_some() {
            return;
        }
        state.maps.pending = Some(Request::Change {
            request: Uuid::new_v4(),
            revision,
            command,
        });
    }
    retry(shared, context);
}

fn retry(shared: Rc<RefCell<super::State>>, context: egui::Context) {
    let Some((client, request, generation)) = ({
        let state = shared.borrow();
        state
            .maps
            .client
            .clone()
            .zip(state.maps.pending.clone())
            .map(|(client, request)| (client, request, state.maps.generation))
    }) else {
        return;
    };
    spawn_local(async move {
        let reply = client.request(request).await;
        let mut state = shared.borrow_mut();
        if state.maps.generation != generation {
            return;
        }
        match reply {
            Ok(reply) => {
                // Watch is authoritative; a retained retry reply can predate its latest snapshot.
                state.maps.pending = None;
                state.maps.error = reply.err().map(ClientError::Request);
            }
            Err(error) => {
                drop(state);
                fail(&shared, &context, generation, error.to_string());
            }
        }
        context.request_repaint();
    });
}

fn fail(
    shared: &Rc<RefCell<super::State>>,
    context: &egui::Context,
    generation: Uuid,
    error: String,
) {
    let mut state = shared.borrow_mut();
    if state.maps.generation == generation {
        state.maps.error = Some(ClientError::Connection(error));
        state.maps.client = None;
        let shared = Rc::clone(shared);
        let context = context.clone();
        context.request_repaint();
        spawn_local(async move {
            super::wait_milliseconds(2_000).await;
            let retry = {
                let state = shared.borrow();
                state.maps.generation == generation
                    && state.maps.client.is_none()
                    && state.client.is_some()
            };
            if retry {
                reconnect(shared, context);
            }
        });
    }
}
