use gallery::prelude::*;
use garmin_ui::radio::Choice;

scene_meta! { title: "Components / Input / Radio" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let examples = [
        ("Unselected", false, true),
        ("Selected", true, true),
        ("Disabled", false, false),
        ("Disabled selected", true, false),
    ];
    ctx.matrix_with(ui, &[egui::vec2(192.0, 0.0); 4], |ctx, ui, i| {
        let (label, initial, enabled) = examples[i];
        globals.example(ctx, ui, label, 192.0, |ui| {
            let id = ui.id().with("selected");
            let selected = ui.data(|data| data.get_temp(id)).unwrap_or(initial);
            if Choice::new("Metric", (), "units.metric")
                .enabled(enabled)
                .show(ui, selected)
                .clicked()
            {
                ui.data_mut(|data| data.insert_temp(id, true));
            }
        });
    });
}
