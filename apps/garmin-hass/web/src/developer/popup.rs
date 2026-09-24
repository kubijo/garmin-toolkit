//! A dedicated egui runner: controls stay usable while the app canvas is automated.
use super::protocol::Snapshot;
use crate::window_client::Client;
use eframe::egui::{self, Context};
use garmin_service_api::{
    ApplicationService as _,
    logging::{LogService as _, LogServiceClient},
};
use garmin_ui::developer::AutomationRequest;
use garmin_ui::developer::{Automation, state};
use std::{cell::RefCell, rc::Rc, time::Duration};

pub fn create(context: Context, session: &str) -> Result<Box<dyn eframe::App>, String> {
    Ok(Box::new(Tools::new(context, session)?))
}

struct Tools {
    connection: Client<AutomationRequest, Snapshot>,
    client: Rc<RefCell<Option<LogServiceClient>>>,
    first_frame: bool,
}

impl Tools {
    fn new(context: Context, session: &str) -> Result<Self, String> {
        let connection = Client::new(context.clone(), session)?;
        state(&context)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remote_automation = Some(Automation::default());
        let client = Rc::new(RefCell::new(None));
        connect_logs(context, client.clone());
        Ok(Self {
            connection,
            client,
            first_frame: true,
        })
    }

    fn receive(&mut self, context: &Context, now: f64) {
        for snapshot in self.connection.receive(now) {
            apply_snapshot(context, snapshot);
        }
        let handle = state(context);
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(automation) = &mut state.remote_automation {
            automation.connected = self.connection.link.connected(now);
            automation.pending = self.connection.link.pending();
            if let Some(error) = &self.connection.link.error {
                automation.error = Some(error.clone());
            }
        }
    }

    fn send_commands(&mut self, context: &Context, now: f64) {
        let handle = state(context);
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(automation) = &mut state.remote_automation else {
            return;
        };
        for request in std::mem::take(&mut automation.requests) {
            self.connection.send(request, now);
        }
    }
}

impl eframe::App for Tools {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            super::super::hide_loading_overlay();
        }
        let context = ui.ctx().clone();
        let now = ui.input(|input| input.time);
        self.receive(&context, now);
        garmin_ui::developer::contents(
            ui,
            &mut state(&context)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.send_commands(&context, now);
        super::update_logs(&context, self.client.borrow().clone());
        context.request_repaint_after(Duration::from_millis(250));
    }
}

fn apply_snapshot(context: &Context, snapshot: Snapshot) {
    if context.global_style().visuals.dark_mode != snapshot.dark {
        context.set_visuals(if snapshot.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
    }
    let handle = state(context);
    let mut state = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.remote_automation = Some(snapshot.automation);
    state.viewport = snapshot.viewport;
    state.scale = snapshot.scale;
    state.debug = snapshot.debug;
    if let Some(renderer) = snapshot.renderer {
        state.debug.push('\n');
        state.debug.push_str(&renderer);
    }
}

fn connect_logs(context: Context, shared: Rc<RefCell<Option<LogServiceClient>>>) {
    wasm_bindgen_futures::spawn_local(async move {
        loop {
            let result: Result<(), String> = async {
                let app = super::super::connect_client()
                    .await
                    .map_err(|error| error.to_string())?;
                let logs = app.logs().await.map_err(|error| error.to_string())?;
                *shared.borrow_mut() = Some(logs.clone());
                garmin_ui::developer::reconnect(&context);
                context.request_repaint();
                loop {
                    super::super::wait_milliseconds(1_000).await;
                    super::super::heartbeat(&app)
                        .await
                        .map_err(|error| error.to_string())?;
                    let batch = super::browser_logs::pending();
                    if !batch.records.is_empty() {
                        let report = logs
                            .ingest(batch.records.clone())
                            .await
                            .map_err(|error| error.to_string())??;
                        super::browser_logs::acknowledge(&batch, report.rejected);
                    }
                }
            }
            .await;
            *shared.borrow_mut() = None;
            let handle = state(&context);
            {
                let mut state = handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.connected = false;
                state.log_error = result.err();
                state.generation += 1;
            }
            context.request_repaint();
            super::super::wait_milliseconds(1_000).await;
        }
    });
}
