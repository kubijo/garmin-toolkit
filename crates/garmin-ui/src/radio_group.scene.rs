use gallery::prelude::*;
use garmin_ui::radio_group;

scene_meta! { title: "Components / Input / Radio group" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let examples = [
        ("Inline", 320.0, true),
        ("Narrow", 144.0, true),
        ("Disabled group", 320.0, false),
    ];
    ctx.matrix_with(
        ui,
        &examples.map(|(_, width, _)| egui::vec2(width, 0.0)),
        |ctx, ui, i| {
            let (label, width, enabled) = examples[i];
            globals.example(ctx, ui, label, width, |ui| {
                let id = ui.id().with("selected");
                let mut value = ui.data_mut(|data| *data.get_temp_mut_or(id, 0));
                if let Some(changed) = radio_group::show(
                    ui,
                    value,
                    &[
                        radio_group::Choice::new("Metric", 0, "units.metric"),
                        radio_group::Choice::new("Imperial", 1, "units.imperial"),
                    ],
                    radio_group::Props {
                        label: "Unit system",
                        helper: Some("Controls displayed distances and elevations."),
                        enabled,
                    },
                ) {
                    value = changed;
                }
                ui.data_mut(|data| data.insert_temp(id, value));
            });
        },
    );
}
