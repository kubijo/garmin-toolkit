use gallery::prelude::*;
use garmin_ui::Size;
use garmin_ui::input;

scene_meta! { title: "Desktop / Components / Input" }

struct SceneProps {
    value: String,
    state: usize,
    size: Size,
    disabled: bool,
    width: f32,
}

#[scene(default)]
fn text_playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let mut props = SceneProps {
        value: ctx.text("value", "Alex Rider"),
        state: ctx.buttons("message", &["none", "helper", "error"], 1),
        size: size(ctx.buttons("size", &["small", "medium", "large"], 1)),
        disabled: ctx.toggle("disabled", false),
        width: ctx.slider("width", 360.0, 220.0, 640.0, 1.0),
    };

    stage!(ctx, ui, |ui| {
        ui.set_width(props.width);
        input::show(
            ui,
            &mut props.value,
            input::Props::new("Profile name")
                .placeholder("Enter a name")
                .message(message(props.state))
                .size(props.size)
                .disabled(props.disabled),
        );
    });
}

#[scene]
fn text_states(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        let mut empty = String::new();
        input::show(
            ui,
            &mut empty,
            input::Props::new("Profile name")
                .placeholder("Enter a name")
                .message(input::Message::Helper(
                    "Shown to other people using this app",
                )),
        );
        ui.add_space(16.0);

        let mut invalid = "Alex".to_owned();
        input::show(
            ui,
            &mut invalid,
            input::Props::new("Profile name")
                .placeholder("Enter a name")
                .message(input::Message::Error("That name is already in use")),
        );
        ui.add_space(16.0);

        let mut disabled = "Existing profile".to_owned();
        input::show(
            ui,
            &mut disabled,
            input::Props::new("Profile name")
                .placeholder("Enter a name")
                .disabled(true),
        );
    });
}

#[scene]
fn text_sizes(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        for (label, size) in [
            ("Small", Size::Small),
            ("Medium", Size::Medium),
            ("Large", Size::Large),
        ] {
            let mut value = label.to_owned();
            input::show(
                ui,
                &mut value,
                input::Props::new("Profile name")
                    .placeholder("Enter a name")
                    .size(size),
            );
            ui.add_space(16.0);
        }
    });
}

const fn message(index: usize) -> Option<input::Message<'static>> {
    match index {
        1 => Some(input::Message::Helper(
            "Shown to other people using this app",
        )),
        2 => Some(input::Message::Error("That name is already in use")),
        _ => None,
    }
}

const fn size(index: usize) -> Size {
    match index {
        0 => Size::Small,
        2 => Size::Large,
        _ => Size::Medium,
    }
}
