use gallery::prelude::*;
use garmin_ui::{
    action_row,
    action_row::Placement,
    button::{Kind, Props},
};

scene_meta! { title: "Components / Layout / Action row" }

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let examples = [
        ("Leading", Placement::Leading, 400.0),
        ("Trailing", Placement::Trailing, 400.0),
        ("Stacked", Placement::Trailing, 176.0),
    ];
    let sizes = examples.map(|(_, _, width)| egui::vec2(width, 0.0));
    ctx.matrix_with(ui, &sizes, |ctx, ui, index| {
        let (stage, placement, width) = examples[index];
        globals.example(ctx, ui, stage, width, |ui| {
            let chosen = action_row::show(
                ui,
                placement,
                [
                    (
                        "review",
                        "Review selection",
                        Props::new("Review selection", Kind::Primary),
                    ),
                    (
                        "edit",
                        "Edit selection",
                        Props::new("Edit selection", Kind::Secondary),
                    ),
                ],
            );
            let id = ui.id().with("action");
            if let Some(chosen) = chosen {
                ui.data_mut(|data| data.insert_temp(id, chosen));
            }
            if let Some(chosen) = ui.data(|data| data.get_temp::<&str>(id)) {
                ui.label(format!("Selected action: {chosen}"));
            }
        });
    });
}
