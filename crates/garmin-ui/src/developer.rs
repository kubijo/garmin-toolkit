//! Shared developer tools. Hosts own transports, files, and native window actions.
use egui::{Context, Id, Rect, Ui};
use garmin_model::logging::{Cursor, Filter, Record};
use garmin_service_api::logging::{LogService, LogServiceClient};
use std::sync::{Arc, Mutex};

#[cfg(any(test, feature = "automation"))]
mod automation;
#[cfg(any(test, feature = "automation"))]
pub use automation::{Automation, AutomationRequest};
mod log_export;
mod logs_view;
mod native;
mod panel;

#[derive(Clone, Debug)]
pub enum Request {
    Subscribe {
        generation: u64,
        filter: Filter,
        cursor: Option<Cursor>,
    },
    Export(Filter),
    OpenFolder,
    StartServer,
    StopServer,
}

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "window visibility, host type, connection, tail pause and retention gap are independent"
)]
pub struct State {
    pub open: bool,
    pub focus_requested: bool,
    pub native: bool,
    #[cfg(any(test, feature = "automation"))]
    pub remote_automation: Option<Automation>,
    pub server: Option<String>,
    pub server_error: Option<String>,
    pub debug: String,
    pub viewport: [f32; 2],
    pub scale: f32,
    pub logs: Vec<Record>,
    pub filter: Filter,
    pub cursor: Option<Cursor>,
    pub generation: u64,
    pub connected: bool,
    pub paused: bool,
    pub log_error: Option<String>,
    pub gap: bool,
    pub export: Option<String>,
    pub export_error: Option<String>,
    pub requests: Vec<Request>,
}

pub type Handle = Arc<Mutex<State>>;

#[must_use]
pub fn state(context: &Context) -> Handle {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Handle>(Id::new("developer-tools"))
            .clone()
    })
}

pub fn header_button(ui: &mut Ui, rect: Rect) -> egui::Response {
    let response = ui
        .scope_builder(
            egui::UiBuilder::new()
                .id_salt("developer-tools-button")
                .max_rect(rect)
                .layout(egui::Layout::centered_and_justified(
                    egui::Direction::LeftToRight,
                )),
            |ui| {
                crate::button::IconProps {
                    label: "Developer tools",
                    icon: crate::icons::CODE,
                    kind: crate::button::Kind::Ghost,
                    size: crate::Size::Large,
                    enabled: true,
                }
                .show_with_dimension(ui, rect.width().min(rect.height()))
            },
        )
        .inner;
    crate::semantics::target(ui, &response, "developer.tools");
    if response.clicked() {
        let handle = state(ui.ctx());
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.open = true;
        state.focus_requested = true;
    }
    response
}

pub fn take_requests(context: &Context) -> Vec<Request> {
    std::mem::take(
        &mut state(context)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .requests,
    )
}

pub fn reconnect(context: &Context) {
    let handle = state(context);
    let mut state = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.generation += 1;
    if !state.paused {
        let request = Request::Subscribe {
            generation: state.generation,
            filter: state.filter.clone(),
            cursor: state.cursor,
        };
        state.requests.push(request);
    }
}

pub fn show(context: &Context, native: bool, intl: &garmin_i18n::Intl) {
    if native {
        self::native::show(context, intl);
        return;
    }
    let handle = state(context);
    {
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.native = native;
        let viewport = context.viewport_rect();
        state.viewport = [viewport.width(), viewport.height()];
        state.scale = context.pixels_per_point();
        if !state.open {
            return;
        }
    }
    // Embedded previews share the app's viewport and would cover scenario targets.
    // Retain `open` so the panel returns with the report after the run ends.
    #[cfg(any(test, feature = "automation"))]
    if context
        .plugin_opt::<crate::automation::Driver>()
        .is_some_and(|plugin| plugin.lock().running())
    {
        return;
    }
    let mut open = true;
    egui::Window::new("Developer tools")
        .open(&mut open)
        .collapsible(true)
        .default_size([620.0, 580.0])
        .show(context, |ui| {
            contents(
                ui,
                &mut handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        });
    if !open {
        handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open = false;
    }
}

pub fn contents(ui: &mut Ui, state: &mut State) {
    panel::show(ui, state);
}

/// Consume the same log RPC from either host; transports and scheduling remain host-owned.
pub async fn logs(context: Context, client: LogServiceClient, request: Request) {
    let handle = state(&context);
    let (generation, export) = match &request {
        Request::Subscribe { generation, .. } => (*generation, false),
        Request::Export(_) => (0, true),
        _ => return,
    };
    if export {
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.export = None;
        state.export_error = None;
    }
    let stream = match request {
        Request::Subscribe { filter, cursor, .. } => client.subscribe(filter, cursor).await,
        Request::Export(filter) => client.export(filter).await,
        _ => unreachable!(),
    };
    let mut output = log_export::Export::default();
    let result = async {
        let mut stream = stream.map_err(|error| error.to_string())?;
        while let Some(batch) = stream.recv().await.map_err(|error| error.to_string())? {
            let mut state = handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !export && state.generation != generation {
                return Ok::<_, String>(());
            }
            let changed = export
                || !state.connected
                || batch.gap
                || batch.error != state.log_error
                || !batch.records.is_empty();
            if export {
                output.push(&batch)?;
            } else {
                if state
                    .cursor
                    .is_some_and(|cursor| cursor.epoch != batch.cursor.epoch)
                {
                    state.logs.clear();
                }
                state.connected = true;
                state.cursor = Some(batch.cursor);
                state.gap |= batch.gap;
                state.log_error = batch.error;
                state.logs.extend(batch.records);
                let excess = state.logs.len().saturating_sub(2048);
                state.logs.drain(..excess);
            }
            if changed {
                context.request_repaint();
                context
                    .request_repaint_of(egui::ViewportId::from_hash_of("developer-tools-window"));
            }
        }
        if !export {
            let mut state = handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.generation == generation {
                state.connected = false;
            }
        }
        if export {
            handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .export = Some(output.finish());
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if export {
            state.export = None;
            state.export_error = Some(error);
        } else {
            state.log_error = Some(error);
            state.connected = false;
        }
    }
    context.request_repaint();
}
