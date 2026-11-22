//! Subscribe to the device host. Reopening a subscription never replays a write.

use eframe::egui;
use garmin_i18n::Intl;
use garmin_service_api::{
    ApplicationService as _,
    firmware::{Action, FirmwareService as _, FirmwareServiceClient, Phase, State as Snapshot},
};
use std::{cell::RefCell, rc::Rc, time::Duration};
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;

type Shared = Rc<RefCell<super::State>>;

#[derive(Default)]
pub(super) struct Controller {
    device: Option<String>,
    generation: Uuid,
    client: Option<FirmwareServiceClient>,
    snapshot: Option<Snapshot>,
    pending: bool,
    error: Option<String>,
}

impl Controller {
    pub(super) fn take_route(&mut self) -> Self {
        Self {
            device: self.device.take(),
            ..Self::default()
        }
    }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    intl: &Intl,
    shared: &Shared,
    device: &str,
) -> Option<(Uuid, Action)> {
    let host = shared.borrow();
    let page = &host.firmware;
    if let Some(error) = &page.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let Some(snapshot) = page
        .snapshot
        .as_ref()
        .filter(|_| page.device.as_deref() == Some(device))
    else {
        ui.spinner();
        return None;
    };
    ui.add_enabled_ui(
        host.client.is_some() && page.client.is_some() && !page.pending,
        |ui| {
            garmin_ui::firmware::show(ui, intl, snapshot).map(|action| (snapshot.revision, action))
        },
    )
    .inner
}

pub(super) fn open(shared: Shared, context: egui::Context, device: String) {
    let generation = Uuid::new_v4();
    let client = {
        let mut host = shared.borrow_mut();
        host.firmware = Controller {
            device: Some(device.clone()),
            generation,
            ..Controller::default()
        };
        host.client.clone()
    };
    let Some(client) = client else {
        return;
    };
    spawn_local(async move {
        let result = async {
            let service = client
                .firmware(device)
                .await
                .map_err(|error| error.to_string())??;
            let mut watch = service.watch().await.map_err(|error| error.to_string())?;
            loop {
                {
                    let mut host = shared.borrow_mut();
                    if host.firmware.generation != generation {
                        return Ok::<(), String>(());
                    }
                    let snapshot = watch
                        .borrow_and_update()
                        .map_err(|error| error.to_string())?
                        .clone();
                    if snapshot.phase == Phase::Disconnected {
                        host.firmware.snapshot = Some(snapshot);
                        return Ok(());
                    }
                    host.firmware.client = Some(service.clone());
                    host.firmware.snapshot = Some(snapshot);
                }
                context.request_repaint();
                watch.changed().await.map_err(|error| error.to_string())?;
            }
        }
        .await;
        {
            let mut host = shared.borrow_mut();
            if host.firmware.generation != generation {
                return;
            }
            host.firmware.error = result.err();
            host.firmware.pending = false;
            host.firmware.client = None;
        }
        context.request_repaint();
        super::wait_for(Duration::from_secs(2)).await;
        let retry = {
            let host = shared.borrow();
            host.firmware.generation == generation && host.client.is_some()
        };
        if retry {
            reconnect(shared, context);
        }
    });
}

pub(super) fn reconnect(shared: Shared, context: egui::Context) {
    let device = {
        let host = shared.borrow();
        host.firmware
            .device
            .clone()
            .filter(|key| host.snapshots.iter().any(|device| &device.key == key))
    };
    if let Some(device) = device {
        open(shared, context, device);
    }
}

pub(super) fn submit(shared: Shared, context: egui::Context, revision: Uuid, action: Action) {
    let (client, generation) = {
        let mut host = shared.borrow_mut();
        let page = &mut host.firmware;
        let Some(client) = page.client.clone().filter(|_| !page.pending) else {
            return;
        };
        page.pending = true;
        (client, page.generation)
    };
    spawn_local(async move {
        let reply = client
            .act(revision, action)
            .await
            .map_err(|error| error.to_string())
            .and_then(|reply| reply);
        let mut host = shared.borrow_mut();
        if host.firmware.generation == generation && host.firmware.client.is_some() {
            host.firmware.pending = false;
            host.firmware.error = reply.err();
        }
        context.request_repaint();
    });
}
