use eframe::egui::{Context, Pos2};

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
