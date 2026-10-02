use gallery::prelude::*;
use garmin_i18n::format_message;

scene_meta! { title: "Components / Navigation / Accordion" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let open = ctx.toggle("open", false);
    let width = ctx.slider("width", 560.0, 288.0, 880.0, 1.0);
    let intl = globals.intl();
    let details = format_message!(intl, default_message: "Details");
    let plan = format_message!(intl, default_message: "Plan");
    stage!(ctx, ui, globals.stage((width, 240.0)), |ui| {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        ui.push_id(open, |ui| {
            garmin_ui::accordion::show(
                ui,
                &garmin_ui::accordion::Props {
                    id: "accordion.details",
                    label: &details,
                    default_open: open,
                    inline_padding: 16,
                },
                |ui| {
                    ui.label(&plan);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "8e51d6f093b27a4c6d9e02f1a748c35be690d124f7a3c85d21906e4b5f8a732c",
                            )
                            .monospace()
                            .size(12.0),
                        )
                        .wrap(),
                    );
                },
            );
        });
    });
}
