use gallery::prelude::*;
use garmin_ui::checkbox;

scene_meta! { title: "Components / Input / Checkbox" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let examples = [
        ("Unchecked", Some(false), true),
        ("Checked", Some(true), true),
        ("Mixed", None, true),
        ("Disabled unchecked", Some(false), false),
        ("Disabled checked", Some(true), false),
        ("Disabled mixed", None, false),
    ];
    ctx.matrix_with(ui, &[egui::vec2(192.0, 0.0); 6], |ctx, ui, i| {
        let (label, initial, enabled) = examples[i];
        globals.example(ctx, ui, label, 192.0, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let id = ui.id().with("selection");
                let mut state = ui.data(|data| data.get_temp(id)).unwrap_or(initial);
                if checkbox::show_mixed(ui, &mut state, "Include maps").changed() {
                    ui.data_mut(|data| data.insert_temp(id, state));
                }
            });
        });
    });
}
