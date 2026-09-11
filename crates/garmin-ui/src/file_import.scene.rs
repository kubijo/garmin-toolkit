use gallery::prelude::*;
use garmin_ui::file_import;

scene_meta! { title: "Desktop / Import" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let drop_active = ctx.toggle("drop active", false);
    let enabled = ctx.toggle("enabled", true);
    stage!(ctx, ui, |ui| {
        ui.set_width(720.0);
        let _ = file_import::show(
            ui,
            &file_import::Props {
                title: "Import FIT activities",
                description: "Drop FIT files or folders here",
                files_label: "Choose files",
                folder_label: "Choose a folder",
                drop_active,
                enabled,
            },
        );
    });
}

#[scene]
fn narrow(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(320.0);
        let _ = file_import::show(
            ui,
            &file_import::Props {
                title: "Import FIT activities",
                description: "Drop FIT files or folders here",
                files_label: "Choose files",
                folder_label: "Choose a folder",
                drop_active: false,
                enabled: true,
            },
        );
    });
}
