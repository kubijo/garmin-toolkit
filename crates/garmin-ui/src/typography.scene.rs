use gallery::prelude::*;
use garmin_ui::typography;

scene_meta! { title: "Components / Typography" }

#[scene(default)]
fn body(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 480.0, 288.0, 640.0, 1.0);
    let text = ctx.text(
        "text",
        "A backup includes all profiles, activities, original files, routes, and pictures. Device pairing and cloud credentials are excluded.",
    );
    stage!(ctx, ui, globals.stage((width, 240.0)), |ui| {
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.heading("Body copy");
                typography::body(ui, &text);
                ui.add_space(16.0);
                ui.label(typography::body_text("Body text inside a standard label."));
            });
    });
}
