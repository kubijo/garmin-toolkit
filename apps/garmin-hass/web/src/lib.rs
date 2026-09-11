#![cfg(target_arch = "wasm32")]

use byte_unit::{Byte, UnitType};
use bytes::Bytes;
use eframe::egui::{ScrollArea, Ui};
use futures_util::{SinkExt as _, StreamExt as _, future};
use garmin_i18n::{Intl, Language, Translations, format_message};
use garmin_model::device::{DeviceStorageState, StorageCapacity};
use garmin_service_api::{DeviceService, DeviceServiceClient, DeviceSnapshot, InspectionState};
use garmin_ui::{capacity, device, icons};
use remoc::prelude::*;
use std::{cell::RefCell, fmt, io, rc::Rc};
use wasm_bindgen::{JsCast as _, prelude::*};
use wasm_bindgen_futures::spawn_local;
use websocket_web::{Msg, WebSocket};

const CANVAS_ID: &str = "garmin-toolkit";

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let canvas = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(CANVAS_ID))
        .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| js_error("the application canvas is missing"))?;

    spawn_local(async move {
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|creation| {
                    garmin_ui::install(&creation.egui_ctx);
                    Ok(Box::new(App::new(creation.egui_ctx.clone())?))
                }),
            )
            .await;
        if let Err(error) = result {
            show_startup_error(&format!(
                "Could not start Garmin Toolkit: {}",
                js_reason(&error)
            ));
        }
    });
    Ok(())
}

struct App {
    intl: Intl,
    shared: Rc<RefCell<State>>,
    first_frame: bool,
}

impl App {
    fn new(context: eframe::egui::Context) -> Result<Self, garmin_i18n::Error> {
        let intl = Translations::bundled()?.formatter(Language::English)?;
        let shared = Rc::new(RefCell::new(State::default()));
        spawn_connection(Rc::clone(&shared), context);
        Ok(Self {
            intl,
            shared,
            first_frame: true,
        })
    }

    fn show_device(&self, ui: &mut Ui, snapshot: &DeviceSnapshot) {
        let status = match snapshot.inspection {
            InspectionState::Running => {
                format_message!(&self.intl, default_message: "Inspecting…")
            }
            InspectionState::Ready => format_message!(&self.intl, default_message: "Ready"),
            InspectionState::Failed => {
                format_message!(&self.intl, default_message: "Inspection failed")
            }
        };
        let status_label = format_message!(&self.intl, default_message: "Status");
        let identifier_label = format_message!(&self.intl, default_message: "Device ID");
        let software_label = format_message!(&self.intl, default_message: "Software");
        let identifier = snapshot.identifier.map(|value| value.to_string());
        let software = snapshot
            .software_version
            .map(|value| format!("{}.{:02}", value / 100, value % 100));
        let storages = snapshot
            .storages
            .iter()
            .map(|storage| StorageView::new(storage, &self.intl))
            .collect::<Vec<_>>();
        let storage_props = storages.iter().map(StorageView::props).collect::<Vec<_>>();
        device::show(
            ui,
            &device::Props {
                name: &snapshot.name,
                connection: "USB/MTP",
                identifier: identifier.as_deref(),
                software: software.as_deref(),
                status: &status,
                status_label: &status_label,
                identifier_label: &identifier_label,
                software_label: &software_label,
                transfers_label: "",
                transfers: &[],
                storages: &storage_props,
                icon: icons::WATCH,
            },
        );
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            remove_loading_overlay();
        }
        ui.heading("Garmin Toolkit");
        ui.label(format_message!(&self.intl, default_message: "Devices"));
        ui.add_space(16.0);

        let (snapshots, error, connected) = {
            let state = self.shared.borrow();
            (
                state.snapshots.clone(),
                state.error.clone(),
                state.client.is_some(),
            )
        };
        if let Some(error) = error.as_deref() {
            device::show_collection_state(ui, device::CollectionState::Error(error));
            return;
        }
        if !connected {
            let message = format_message!(
                &self.intl,
                default_message: "Connecting to the device host…",
            );
            device::show_collection_state(ui, device::CollectionState::Connecting(&message));
            return;
        }
        if snapshots.is_empty() {
            let message = format_message!(
                &self.intl,
                default_message: "No Garmin devices are connected.",
            );
            device::show_collection_state(ui, device::CollectionState::Empty(&message));
            return;
        }
        ScrollArea::vertical().show(ui, |ui| {
            for snapshot in &snapshots {
                self.show_device(ui, snapshot);
                ui.add_space(24.0);
            }
        });
    }
}

#[derive(Default)]
struct State {
    client: Option<DeviceServiceClient>,
    snapshots: Vec<DeviceSnapshot>,
    error: Option<String>,
}

struct StorageView {
    label: String,
    detail: String,
    bytes: Option<(u64, u64)>,
}

impl StorageView {
    fn new(storage: &DeviceStorageState, intl: &Intl) -> Self {
        let label = if storage.writable == Some(false) {
            format_message!(
                intl,
                default_message: "{storage} · read-only",
                values: { storage: storage.label.as_str() },
            )
        } else {
            storage.label.clone()
        };
        let bytes = storage
            .capacity
            .bytes()
            .map(|(total, free)| (total - free, total));
        let detail = storage.capacity.bytes().map_or_else(
            || {
                let reason = match &storage.capacity {
                    StorageCapacity::Unavailable { reason } => reason.as_str(),
                    StorageCapacity::Available { .. } => {
                        "The device reported invalid storage totals"
                    }
                };
                format_message!(
                    intl,
                    default_message: "Could not read storage usage: {reason}",
                    values: { reason: reason },
                )
            },
            |(total, free)| {
                format_message!(
                    intl,
                    default_message: "{used} used · {free} free · {total} total",
                    values: {
                        used: format_bytes(total - free),
                        free: format_bytes(free),
                        total: format_bytes(total),
                    },
                )
            },
        );
        Self {
            label,
            detail,
            bytes,
        }
    }

    fn props(&self) -> capacity::Props<'_> {
        capacity::Props {
            label: &self.label,
            detail: &self.detail,
            bytes: self.bytes,
        }
    }
}

fn spawn_connection(shared: Rc<RefCell<State>>, context: eframe::egui::Context) {
    spawn_local(async move {
        match connect().await {
            Ok((client, mut snapshots)) => {
                let initial = snapshots.borrow_and_update().map(|value| value.clone());
                match initial {
                    Ok(initial) => {
                        let mut state = shared.borrow_mut();
                        state.client = Some(client);
                        state.snapshots = initial;
                    }
                    Err(error) => shared.borrow_mut().error = Some(error.to_string()),
                }
                context.request_repaint();
                loop {
                    if let Err(error) = snapshots.changed().await {
                        shared.borrow_mut().error = Some(error.to_string());
                        context.request_repaint();
                        break;
                    }
                    match snapshots.borrow_and_update() {
                        Ok(value) => shared.borrow_mut().snapshots = value.clone(),
                        Err(error) => shared.borrow_mut().error = Some(error.to_string()),
                    }
                    context.request_repaint();
                }
            }
            Err(error) => {
                shared.borrow_mut().error = Some(error.to_string());
                context.request_repaint();
            }
        }
    });
}

async fn connect() -> Result<
    (
        DeviceServiceClient,
        remoc::rch::watch::Receiver<Vec<DeviceSnapshot>>,
    ),
    ConnectError,
> {
    let websocket = WebSocket::connect(&websocket_url()?)
        .await
        .map_err(ConnectError::transport)?;
    let (websocket_tx, websocket_rx) = websocket.into_split();
    let transport_tx = websocket_tx
        .with(|packet: Bytes| future::ready(Ok::<_, io::Error>(Msg::Binary(packet.into()))));
    let transport_rx = websocket_rx.filter_map(|message| {
        future::ready(match message {
            Ok(Msg::Binary(packet)) => Some(Ok(Bytes::from(packet))),
            Ok(Msg::Text(_)) => None,
            Err(error) => Some(Err(error)),
        })
    });
    let client: DeviceServiceClient =
        remoc::Connect::framed(remoc::Cfg::default(), transport_tx, transport_rx)
            .consume()
            .await
            .map_err(ConnectError::remoc)?;
    let snapshots = client.watch().await.map_err(ConnectError::call)?;
    Ok((client, snapshots))
}

fn websocket_url() -> Result<String, ConnectError> {
    let href = web_sys::window()
        .ok_or_else(|| ConnectError::location("browser window is unavailable"))?
        .location()
        .href()
        .map_err(|_| ConnectError::location("browser location is unavailable"))?;
    let (base, _) = href
        .rsplit_once('/')
        .ok_or_else(|| ConnectError::location("browser URL has no directory"))?;
    let base = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            base.strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .ok_or_else(|| ConnectError::location("browser URL is not HTTP or HTTPS"))?;
    Ok(format!("{base}/remoc"))
}

#[derive(Debug)]
struct ConnectError(String);

impl ConnectError {
    fn transport(error: impl fmt::Display) -> Self {
        Self(format!("WebSocket connection failed: {error}"))
    }

    fn remoc(error: impl fmt::Display) -> Self {
        Self(format!("Device service negotiation failed: {error}"))
    }

    fn call(error: impl fmt::Display) -> Self {
        Self(format!("Device service call failed: {error}"))
    }

    fn location(reason: &str) -> Self {
        Self(reason.to_owned())
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn format_bytes(bytes: u64) -> String {
    format!(
        "{:.2}",
        Byte::from_u64(bytes).get_appropriate_unit(UnitType::Decimal)
    )
}

fn remove_loading_overlay() {
    if let Some(overlay) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("loading"))
    {
        overlay.remove();
    }
}

fn show_startup_error(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
    if let Some(overlay) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("loading-message"))
    {
        overlay.set_text_content(Some(message));
    }
}

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

fn js_reason(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}
