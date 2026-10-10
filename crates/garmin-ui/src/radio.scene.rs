use gallery::prelude::*;
use garmin_ui::radio;

scene_meta! { title: "Components / Input / Radio group" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let disabled = ctx.toggle("disabled", false);
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.set_width(320.0);
        let id = ui.id().with("selected");
        let mut value = ui.data_mut(|data| *data.get_temp_mut_or(id, 0));
        if let Some(changed) = radio::show(
            ui,
            value,
            &[
                radio::Choice::new("Metric", 0, "units.metric"),
                radio::Choice::new("Imperial", 1, "units.imperial"),
            ],
            radio::Props {
                label: "Unit system",
                helper: Some("Controls displayed distances and elevations."),
                enabled: !disabled,
            },
        ) {
            value = changed;
        }
        ui.data_mut(|data| data.insert_temp(id, value));
    });
}
