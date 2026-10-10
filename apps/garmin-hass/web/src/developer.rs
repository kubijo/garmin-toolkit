//! Browser transport adapter for shared tools and centralized diagnostics.
use eframe::egui::Context;
use garmin_service_api::logging::{LogService, LogServiceClient};
use wasm_bindgen::prelude::*;

mod browser_logs;
mod host;
mod log_buffer;
pub(super) mod popup;
mod protocol;

pub use host::Host;

#[wasm_bindgen(module = "/logging.js")]
extern "C" {
    #[wasm_bindgen(js_name = downloadLogs)]
    fn download_logs(text: &str);
    #[wasm_bindgen(js_name = receiveLog)]
    pub(super) fn receive_log(data: &JsValue) -> bool;
}

pub fn install() {
    browser_logs::install();
}

pub fn update_logs(context: &Context, client: Option<LogServiceClient>) {
    if let Some(client) = client {
        for request in garmin_ui::developer::take_requests(context) {
            wasm_bindgen_futures::spawn_local(garmin_ui::developer::logs(
                context.clone(),
                client.clone(),
                request,
            ));
        }
    }
    if let Some(export) = garmin_ui::developer::state(context)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .export
        .take()
    {
        download_logs(&export);
    }
}

pub fn forward(shared: std::rc::Rc<std::cell::RefCell<super::State>>, context: Context) {
    wasm_bindgen_futures::spawn_local(async move {
        loop {
            super::wait_milliseconds(500).await;
            let client = shared.borrow().logs.clone();
            if let Some(client) = client {
                let batch = browser_logs::pending();
                if !batch.records.is_empty() {
                    match client.ingest(batch.records.clone()).await {
                        Ok(Ok(report)) => browser_logs::acknowledge(&batch, report.rejected),
                        result => {
                            garmin_ui::developer::state(&context)
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .log_error =
                                Some(format!("Could not forward browser diagnostics: {result:?}"));
                            context.request_repaint();
                        }
                    }
                }
            }
        }
    });
}
