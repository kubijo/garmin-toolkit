use gallery::prelude::*;
use garmin_ui::date_input;

scene_meta! { title: "Components / Input / Date" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let disabled = ctx.toggle("disabled", false);
    stage!(ctx, ui, globals.stage((360, 440)), |ui| {
        let id = egui::Id::new("date-input-playground");
        let mut value = ui
            .data(|data| data.get_temp::<String>(id))
            .unwrap_or_default();
        date_input::show(
            ui,
            id,
            &mut value,
            &globals.intl(),
            "Date",
            !disabled,
            jiff::civil::date(2026, 11, 13),
        );
        ui.data_mut(|data| data.insert_temp(id, value));
    });
}
