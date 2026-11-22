use gallery::prelude::*;
use garmin_ui::skeleton::Skeleton;

scene_meta! { title: "Components / Feedback / Skeleton" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 192.0, 8.0, 320.0, 8.0);
    let height = ctx.slider("height", 24.0, 8.0, 96.0, 8.0);
    let diameter = ctx.slider("diameter", 48.0, 8.0, 96.0, 8.0);
    let rectangle = Skeleton::new(egui::vec2(width, height));
    let circle = Skeleton::circle(diameter);
    let examples = [
        ("Animated rectangle", rectangle, true),
        ("Animated circle", circle, true),
        ("Reduced-motion rectangle", rectangle, false),
        ("Reduced-motion circle", circle, false),
    ];
    let stage_width = width.max(diameter).max(160.0) + 32.0;
    let sizes = [egui::vec2(stage_width, 0.0); 4];
    ctx.matrix_with(ui, &sizes, |ctx, ui, i| {
        let (label, shape, animated) = examples[i];
        globals.example(ctx, ui, label, stage_width, |ui| {
            ui.style_mut().animation_time = if animated { 0.1 } else { 0.0 };
            shape.show(ui);
        });
    });
}
