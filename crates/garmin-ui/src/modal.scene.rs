use gallery::prelude::*;
use garmin_ui::{Size as ComponentSize, icons, input, modal};

scene_meta! { title: "Components / Overlays / Modals" }

#[derive(Clone, Copy)]
struct SceneProps {
    size: modal::Size,
    danger: bool,
    description: bool,
    backdrop: Backdrop,
    enabled: bool,
}

#[derive(Clone, Copy)]
enum Backdrop {
    Static,
    Closes,
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let props = SceneProps {
        size: size(ctx.buttons("size", &["small", "medium", "large"], 1)),
        danger: ctx.toggle("danger", false),
        description: ctx.toggle("description", true),
        backdrop: if ctx.toggle("backdrop closes", false) {
            Backdrop::Closes
        } else {
            Backdrop::Static
        },
        enabled: ctx.toggle("primary enabled", true),
    };
    stage!(ctx, ui, |ui| show_dialog(ui, props, 720.0, 520.0));
}

#[scene]
fn form(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        show_dialog(
            ui,
            SceneProps {
                size: modal::Size::Medium,
                danger: false,
                description: true,
                backdrop: Backdrop::Static,
                enabled: true,
            },
            720.0,
            520.0,
        );
    });
}

#[scene]
fn destructive(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        show_dialog(
            ui,
            SceneProps {
                size: modal::Size::Small,
                danger: true,
                description: true,
                backdrop: Backdrop::Static,
                enabled: true,
            },
            640.0,
            420.0,
        );
    });
}

#[scene]
fn quit(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(640.0, 420.0), egui::Sense::hover());
        let mut parent = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let _ = modal::show(
            &mut parent,
            egui::Id::new("gallery-quit"),
            &modal::Props {
                title: "Abort current work and quit?",
                description: Some("The following work is still in progress:"),
                size: modal::Size::Small,
                presentation: modal::Presentation::Contained,
                cancel_label: "Keep working",
                backdrop_closes: Some(false),
                primary: modal::Primary {
                    label: "Abort and quit",
                    icon: Some(icons::POWER),
                    kind: modal::PrimaryKind::Danger,
                    enabled: true,
                },
            },
            |ui| {
                ui.label("• Importing FIT activities");
                ui.add_space(12.0);
                ui.label("Unfinished work will be discarded.");
            },
        );
    });
}

#[scene]
fn narrow(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        show_dialog(
            ui,
            SceneProps {
                size: modal::Size::Medium,
                danger: false,
                description: true,
                backdrop: Backdrop::Static,
                enabled: true,
            },
            320.0,
            520.0,
        );
    });
}

fn show_dialog(ui: &mut Ui, props: SceneProps, width: f32, height: f32) {
    let mut name = "Alex Rider".to_owned();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let mut parent = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let _ = modal::show(
        &mut parent,
        egui::Id::new(("gallery-modal", props.danger)),
        &modal::Props {
            title: if props.danger {
                "Delete this profile?"
            } else {
                "Create a profile"
            },
            description: props.description.then_some(if props.danger {
                "Activities stay in storage, but this profile can no longer access them."
            } else {
                "Profiles keep each person's activities and devices separate."
            }),
            size: props.size,
            presentation: modal::Presentation::Contained,
            cancel_label: "Cancel",
            backdrop_closes: matches!(props.backdrop, Backdrop::Closes).then_some(true),
            primary: modal::Primary {
                label: if props.danger { "Delete" } else { "Create" },
                icon: Some(if props.danger {
                    icons::TRASH
                } else {
                    icons::PLUS
                }),
                kind: if props.danger {
                    modal::PrimaryKind::Danger
                } else {
                    modal::PrimaryKind::Confirm
                },
                enabled: props.enabled,
            },
        },
        |ui| {
            if props.danger {
                ui.label("Alex Rider");
            } else {
                input::show(
                    ui,
                    &mut name,
                    input::Props::new("Profile name")
                        .placeholder("Enter a name")
                        .size(ComponentSize::Medium),
                );
            }
        },
    );
}

const fn size(index: usize) -> modal::Size {
    match index {
        0 => modal::Size::Small,
        2 => modal::Size::Large,
        _ => modal::Size::Medium,
    }
}
