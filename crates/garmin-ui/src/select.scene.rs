use gallery::prelude::*;
use garmin_ui::{Size, images, select};

scene_meta! { title: "Components / Input / Select" }

const CHOICES: &[select::Choice<'_>] = &[
    select::Choice::new("Metric"),
    select::Choice::new("Imperial"),
];

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let mut selected = ctx.buttons("selected", &["metric", "imperial"], 0);
    let disabled = ctx.toggle("disabled", false);
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        let _ = select::show(
            ui,
            egui::Id::new("units"),
            &mut selected,
            CHOICES,
            select::Props::new("Unit system")
                .helper("Controls displayed distances and elevations")
                .disabled(disabled),
        );
    });
}

#[scene]
fn sizes(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        for (label, size) in [
            ("Small", Size::Small),
            ("Medium", Size::Medium),
            ("Large", Size::Large),
        ] {
            let mut selected = 0;
            let _ = select::show(
                ui,
                egui::Id::new(label),
                &mut selected,
                CHOICES,
                select::Props::new(label).size(size),
            );
            ui.add_space(16.0);
        }
    });
}

#[scene]
fn leading_images(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let mut selected = ctx.buttons("selected", &["english", "czech"], 0);
    let choices = [
        select::Choice::new("English").image(images::UNITED_KINGDOM),
        select::Choice::new("Čeština").image(images::CZECHIA),
    ];
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        let _ = select::show(
            ui,
            egui::Id::new("languages"),
            &mut selected,
            &choices,
            select::Props::new("Language"),
        );
    });
}
