use gallery::prelude::*;
use garmin_ui::summary_table;
use garmin_ui::summary_table::{Cell, Column};

scene_meta! { title: "Components / Content / Summary table" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 480.0, 240.0, 800.0, 8.0);
    let progress = ctx.slider("progress (%)", 72.0, 0.0, 100.0, 1.0);
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        egui::Frame::NONE
            .fill(ui.visuals().panel_fill)
            .show(ui, |ui| {
                ui.set_width(width);
                summary_table::show(
                    ui,
                    "Downloads",
                    [
                        Column::new("File", 3, egui::Align::Min),
                        Column::new("Downloaded", 2, egui::Align::Max),
                    ],
                    [
                        [Cell::new("Time zone map"), Cell::new("569 KiB")],
                        [
                            Cell {
                                text: "Audio prompts".into(),
                                detail: Some("English and Czech".into()),
                                fraction: None,
                            },
                            Cell {
                                text: format!("{progress:.0}%").into(),
                                fraction: Some(progress / 100.0),
                                detail: None,
                            },
                        ],
                    ],
                    Some(("Total download", "83.3 MiB")),
                );
            });
    });
}
