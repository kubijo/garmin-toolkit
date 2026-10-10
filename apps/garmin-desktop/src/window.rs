use eframe::egui::{Context, Pos2};

#[derive(Default)]
struct Background {
    child: bool,
}

impl eframe::egui::plugin::Plugin for Background {
    fn debug_name(&self) -> &'static str {
        "native window background"
    }

    fn on_end_pass(&mut self, ui: &mut eframe::egui::Ui) {
        self.child = ui.ctx().viewport_id() != eframe::egui::ViewportId::ROOT;
    }
}

pub fn install(context: &Context) {
    context.add_plugin(Background::default());
}

pub fn clear_color(context: &Context) -> [f32; 4] {
    // eframe asks after the viewport pass, when Context::viewport_id() is root again.
    if context
        .plugin_opt::<Background>()
        .is_some_and(|state| state.lock().child)
    {
        [0.0; 4]
    } else {
        eframe::egui::Color32::from_rgb(12, 12, 12).to_normalized_gamma_f32()
    }
}

pub fn show_menu(context: &Context, frame: &eframe::Frame, position: Pos2) {
    let key = eframe::egui::Id::new((context.viewport_id(), "desktop-window-menu-pass"));
    let current_frame = context.cumulative_frame_nr();
    let already_sent = context.data_mut(|data| {
        let previous = data.get_temp::<u64>(key);
        data.insert_temp(key, current_frame);
        previous == Some(current_frame)
    });
    if already_sent {
        return;
    }

    if let Some(window) = frame.winit_window() {
        let scale = f64::from(context.pixels_per_point());
        window.show_window_menu(winit::dpi::PhysicalPosition::new(
            f64::from(position.x) * scale,
            f64::from(position.y) * scale,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{RawInput, ViewportId, ViewportInfo};

    #[test]
    fn deferred_child_background_stays_transparent_after_egui_restores_root() {
        let context = Context::default();
        install(&context);
        let child = ViewportId::from_hash_of("test-child");
        for viewport in [ViewportId::ROOT, child, ViewportId::ROOT] {
            let mut input = RawInput {
                viewport_id: viewport,
                ..Default::default()
            };
            input
                .viewports
                .entry(viewport)
                .or_insert_with(|| ViewportInfo {
                    parent: Some(ViewportId::ROOT),
                    ..Default::default()
                });
            let mut output = context.run_ui(input, |_| {});
            output.textures_delta.clear();
            assert_eq!(context.viewport_id(), ViewportId::ROOT);
            let alpha: f32 = if viewport == child { 0.0 } else { 1.0 };
            assert_eq!(clear_color(&context)[3].to_bits(), alpha.to_bits());
        }
    }
}
