//! Progress height is opt-in; the default remains compact and inherits its track color.

use egui_kittest::Harness;
use gallery::eframe::egui;
use garmin_ui::progress;

#[test]
fn progress_respects_optional_height_and_inherited_track() {
    for height in [None, Some(8.0), Some(16.0), Some(24.0)] {
        for value in [
            progress::Value::Indeterminate,
            progress::Value::Determinate {
                completed: 1,
                total: 4,
            },
        ] {
            let track = egui::Color32::from_rgb(77, 88, 99);
            let mut harness = Harness::new_ui(move |ui| {
                ui.visuals_mut().extreme_bg_color = track;
                progress::show(
                    ui,
                    &progress::Props {
                        label: "Progress",
                        detail: None,
                        value,
                        height,
                    },
                );
            });
            harness.run_steps(3);
            let rect = harness
                .output()
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::epaint::Shape::Rect(rect) = &shape.shape
                        && rect.fill == track
                    {
                        Some(rect.rect)
                    } else {
                        None
                    }
                })
                .expect("progress retains the inherited track color");
            assert!((rect.height() - height.unwrap_or(4.0)).abs() < f32::EPSILON);
            assert!(rect.width() > 100.0, "track spans the available width");
        }
    }
}
