use gallery::prelude::*;
use garmin_ui::summary_table;
use garmin_ui::summary_table::{Cell, Column};

scene_meta! { title: "Components / Content / Summary table" }

#[scene]
fn externally_labelled(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.set_width(384.0);
        ui.columns(3, |columns| {
            columns[1].label("First ride");
            columns[2].label("Second ride");
        });
        ui.add_space(8.0);
        summary_table::show_body(
            ui,
            "Ride measurements",
            [
                Column::new("", 1, egui::Align::Min),
                Column::new("First ride", 1, egui::Align::Max),
                Column::new("Second ride", 1, egui::Align::Max),
            ],
            [
                [
                    Cell::new("Distance"),
                    Cell::new("88 km"),
                    Cell::new("47 km"),
                ],
                [Cell::new("Cadence"), Cell::new("—"), Cell::new("79.5 rpm")],
            ],
        );
    });
}

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
