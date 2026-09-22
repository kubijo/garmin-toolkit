//! Demo automation command bridge.

use eframe::egui;
use garmin_ui::automation::{Driver, SCENARIOS};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/ui-automation.js")]
extern "C" {
    #[wasm_bindgen(js_name = installAutomation)]
    fn install_bridge(command: &JsValue);
    #[wasm_bindgen(js_name = launchAutomation, catch)]
    fn launch_bridge(name: &str) -> Result<(), JsValue>;
}

pub(super) fn dispatch_menu(context: &egui::Context) {
    let Some(plugin) = context.plugin_opt::<Driver>() else {
        return;
    };
    let requested = plugin.lock().take_launch_request();
    if let Some(name) = requested
        && let Err(error) = launch_bridge(name)
    {
        plugin.lock().launch_error = Some(super::js_reason(&error));
    }
}

pub(super) fn install(context: &egui::Context) {
    context.add_plugin(Driver::default());
    let context = context.clone();
    let command = Closure::<dyn Fn(String, String) -> String>::new(
        move |operation: String, argument: String| {
            let plugin = context.plugin::<Driver>();
            let mut driver = plugin.lock();
            let result = match operation.as_str() {
                "list" => Ok(serde_json::json!(SCENARIOS)),
                "start" => driver.start(&argument).map(|()| serde_json::Value::Null),
                "cancel" => {
                    driver.cancel(&argument);
                    Ok(serde_json::Value::Null)
                }
                "status" => Ok(serde_json::json!(driver.report())),
                _ => Err("unknown automation command".into()),
            };
            context.request_repaint();
            match result {
                Ok(value) => serde_json::json!({"value": value}),
                Err(error) => serde_json::json!({"error": error}),
            }
            .to_string()
        },
    );
    install_bridge(command.as_ref());
    // Retain the callback for the page lifetime.
    command.forget();
}
