use gallery::prelude::*;
use garmin_ui::{Size, images, select};

scene_meta! { title: "Components / Input / Select" }

const CHOICES: &[select::Choice<'_>] = &[
    select::Choice::new("Low"),
    select::Choice::new("Medium"),
    select::Choice::new("High"),
];

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let mut selected = ctx.buttons("selected", &["low", "medium", "high"], 0);
    let disabled = ctx.toggle("disabled", false);
    let focused = ctx.toggle("focused", false);
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.set_width(360.0);
        let id = egui::Id::new("priority");
        if focused && !disabled {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        let _ = select::show(
            ui,
            id,
            &mut selected,
            CHOICES,
            select::Props::new("Priority")
                .helper("Choose the task priority")
                .disabled(disabled),
        );
    });
}

#[scene]
fn sizes(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
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
fn open_menu(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    stage!(ctx, ui, globals.stage((360, 240)), |ui| {
        let id = egui::Id::new("open-priority");
        egui::Popup::open_id(ui.ctx(), id.with("popup"));
        let mut selected = 0;
        let _ = select::show(
            ui,
            id,
            &mut selected,
            CHOICES,
            select::Props::new("Priority"),
        );
    });
}

#[scene]
fn leading_images(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let mut selected = ctx.buttons("selected", &["automatic", "english", "czech"], 0);
    let choices = [
        select::Choice::new("Automatic"),
        select::Choice::new("English").image(images::UNITED_KINGDOM),
        select::Choice::new("Čeština").image(images::CZECHIA),
    ];
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
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
