use gallery::prelude::*;
use garmin_ui::progress;

scene_meta! { title: "Desktop / Components / Progress" }

#[scene]
fn determinate(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(480.0);
        progress::show(
            ui,
            &progress::Props {
                label: "Importing activities",
                detail: Some("3 of 8 FIT files"),
                value: progress::Value::Determinate {
                    completed: 3,
                    total: 8,
                },
            },
        );
    });
}

#[scene]
fn indeterminate(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(480.0);
        progress::show(
            ui,
            &progress::Props {
                label: "Scanning a folder",
                detail: None,
                value: progress::Value::Indeterminate,
            },
        );
    });
}
