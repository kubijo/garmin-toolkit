use gallery::prelude::*;
use garmin_ui::slider;

scene_meta! { title: "Components / Input / Slider" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let examples = [
        "Editable value",
        "Track only",
        "Named stops",
        "Disabled stops",
    ];
    ctx.matrix_with(ui, &[egui::vec2(280.0, 0.0); 4], |ctx, ui, index| {
        globals.example(ctx, ui, examples[index], 280.0, |ui| {
            let id = ui.id().with("value");
            if index < 2 {
                let mut value = ui.data(|data| data.get_temp(id)).unwrap_or(2.0);
                if index == 0 {
                    slider::editable(ui, "Zoom", &mut value, 1.0..=4.0);
                } else {
                    slider::track(ui, "Zoom", &mut value, 1.0..=4.0);
                }
                ui.data_mut(|data| data.insert_temp(id, value));
            } else {
                let mut step = ui.data(|data| data.get_temp(id)).unwrap_or(1);
                ui.add_enabled_ui(index == 2, |ui| {
                    slider::stops(
                        ui,
                        "speed",
                        "Playback speed",
                        &mut step,
                        ["0.5×", "1×", "2×"],
                    );
                });
                ui.data_mut(|data| data.insert_temp(id, step));
            }
        });
    });
}
