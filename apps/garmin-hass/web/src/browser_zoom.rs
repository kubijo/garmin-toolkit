//! Leave physical Ctrl+wheel to the browser while retaining egui's pinch and map input.

use eframe::egui::{self, Event, RawInput};

fn browser_zoom(event: &Event) -> bool {
    matches!(event, Event::MouseWheel { modifiers, .. } if modifiers.ctrl)
}

pub(super) fn configure(options: &mut eframe::WebOptions) {
    options.should_prevent_default = Box::new(|event| !browser_zoom(event));
}

pub(super) fn prepare_input(context: &egui::Context, input: &mut RawInput) {
    // Eframe maps trackpad pinch to Event::Zoom; keep that gesture for egui.
    // Automation adds its synthetic wheel events after browser input is filtered.
    input.events.retain(|event| !browser_zoom(event));
    garmin_ui::automation::prepare_background_input(context, input);
}
