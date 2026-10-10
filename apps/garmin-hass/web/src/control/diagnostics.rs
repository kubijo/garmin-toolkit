use eframe::egui::{Context, Id};
use garmin_model::diagnostics::{Observation, Update};
use garmin_ui::diagnostics::Journal;
use remoc::rch;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/window-control.js")]
extern "C" {
    #[wasm_bindgen(js_name = publishObservation, catch)]
    fn publish_observation(observation: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = refreshDiagnostics, catch)]
    fn refresh_diagnostics() -> Result<(), JsValue>;
}

fn journal(context: &Context) -> Journal {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Journal>(Id::new("diagnostic-journal"))
            .clone()
    })
}

pub(super) fn install(context: &Context) -> Closure<dyn Fn(String)> {
    let journal = journal(context);
    garmin_ui::diagnostics::install(context, |observation| {
        if let Ok(json) = serde_json::to_string(&observation) {
            let _ = publish_observation(&json);
        }
    });
    Closure::new(move |json: String| {
        if json.len() <= 16 * 1024
            && let Ok(observation) = serde_json::from_str::<Observation>(&json)
        {
            journal.publish(observation);
        }
    })
}

pub(super) fn subscribe(context: &Context, active: Arc<AtomicBool>) -> rch::mpsc::Receiver<Update> {
    let journal = journal(context);
    let (sender, receiver) = rch::mpsc::with_local_buffer(1);
    wasm_bindgen_futures::spawn_local(async move {
        // The reverse subscription can arrive before the registration reply.
        for _ in 0..50 {
            if active.load(Ordering::Acquire) {
                break;
            }
            crate::wait_milliseconds(100).await;
        }
        let mut revision = None;
        while active.load(Ordering::Acquire) && !sender.is_closed() {
            let _ = refresh_diagnostics();
            let mut update = journal.since(revision.unwrap_or(0));
            if revision.is_none() {
                update.changes.clear();
                update.gap = false;
            }
            if revision != Some(update.revision) {
                revision = Some(update.revision);
                if sender.send(update).await.is_err() {
                    break;
                }
            }
            crate::wait_milliseconds(100).await;
        }
    });
    receiver
}
