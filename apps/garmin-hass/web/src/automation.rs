//! Demo automation command bridge.

use eframe::egui;
use garmin_ui::automation::{Driver, ResizeCommand};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/ui-automation.js")]
extern "C" {
    #[wasm_bindgen(js_name = installAutomation)]
    fn install_bridge(command: &JsValue);
    #[wasm_bindgen(js_name = launchAutomation, catch)]
    fn launch_bridge(name: &str) -> Result<(), JsValue>;
}

pub(super) fn launch(name: &str) -> Result<(), String> {
    launch_bridge(name).map_err(|error| super::js_reason(&error))
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
    install_driver(context, Driver::default());
    crate::control::install_window_capture(context);
}

pub(super) fn install_window(context: &egui::Context) {
    install_driver(context, Driver::for_window(egui::ViewportId::ROOT));
}

fn install_driver(context: &egui::Context, driver: Driver) {
    context.add_plugin(driver.with_resize_handler(resize_canvas));
    context.add_plugin(garmin_ui::capture::CapturePlugin::default());
    let context = context.clone();
    let command = Closure::<dyn Fn(String, String) -> String>::new(
        move |operation: String, argument: String| {
            let argument =
                serde_json::from_str(&argument).unwrap_or(serde_json::Value::String(argument));
            let result = garmin_ui::automation::command(&context, &operation, &argument);
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

fn resize_canvas(context: &egui::Context, command: ResizeCommand) -> Result<(), String> {
    let canvas = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(super::CANVAS_ID))
        .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or("the application canvas is missing")?;
    let style = canvas.style();
    let saved_id = egui::Id::new("automation-canvas-sizing");
    let size = match command {
        ResizeCommand::Save => {
            let mut properties = Vec::new();
            for name in ["width", "height"] {
                properties.push((
                    style
                        .get_property_value(name)
                        .map_err(|error| super::js_reason(&error))?,
                    style.get_property_priority(name),
                ));
            }
            context.data_mut(|data| data.insert_temp(saved_id, properties));
            return Ok(());
        }
        ResizeCommand::Restore(_) => {
            let properties = context
                .data(|data| data.get_temp::<Vec<(String, String)>>(saved_id))
                .ok_or("original canvas sizing is missing")?;
            for (name, (value, priority)) in ["width", "height"].into_iter().zip(properties) {
                style
                    .set_property_with_priority(name, &value, &priority)
                    .map_err(|error| super::js_reason(&error))?;
            }
            context.data_mut(|data| data.remove::<Vec<(String, String)>>(saved_id));
            return Ok(());
        }
        ResizeCommand::Set(size) => size,
    };
    // eframe observes CSS size changes and updates the backing surface and input
    // coordinates. Its zoom factor converts logical egui points to CSS pixels.
    let zoom = context.zoom_factor();
    style
        .set_property("width", &format!("{}px", size[0] * zoom))
        .map_err(|error| super::js_reason(&error))?;
    style
        .set_property("height", &format!("{}px", size[1] * zoom))
        .map_err(|error| super::js_reason(&error))?;
    Ok(())
}
