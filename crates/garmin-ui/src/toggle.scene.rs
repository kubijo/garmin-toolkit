use gallery::prelude::*;
use garmin_ui::toggle;

scene_meta! { title: "Components / Input / Toggle" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let examples = [
        ("Off", false, true),
        ("On", true, true),
        ("Disabled off", false, false),
        ("Disabled on", true, false),
    ];
    ctx.matrix_with(ui, &[egui::vec2(192.0, 0.0); 4], |ctx, ui, i| {
        let (label, initial, enabled) = examples[i];
        globals.example(ctx, ui, label, 192.0, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let id = ui.id().with("setting");
                let mut on = ui.data(|data| data.get_temp(id)).unwrap_or(initial);
                if toggle::show(ui, &mut on, "Show surface").changed() {
                    ui.data_mut(|data| data.insert_temp(id, on));
                }
            });
        });
    });
}
