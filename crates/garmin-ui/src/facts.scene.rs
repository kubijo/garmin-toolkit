use gallery::prelude::*;
use garmin_ui::facts;

scene_meta! { title: "Components / Content / Facts" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.set_width(320.0);
        let rows = [
            ("Status", "Ready"),
            ("Device ID", "42530200"),
            ("Software", "18.70"),
        ];
        facts::Table::new(ui, rows.iter().map(|(label, _)| *label)).show(ui, "facts", &rows);
    });
}
