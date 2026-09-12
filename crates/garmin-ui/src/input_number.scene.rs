use gallery::prelude::*;
use garmin_ui::Size;
use garmin_ui::input;

scene_meta! { title: "Components / Input / Number" }

struct SceneProps {
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    size: Size,
    steppers: bool,
    readonly: bool,
    disabled: bool,
    width: f32,
}

#[scene(default)]
fn number_playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let mut props = SceneProps {
        value: ctx.slider("value", 50.0, -100.0, 100.0, 1.0),
        min: ctx.slider("min", -100.0, -500.0, 0.0, 1.0),
        max: ctx.slider("max", 100.0, 0.0, 500.0, 1.0),
        step: ctx.slider("step", 1.0, 0.1, 20.0, 0.1),
        size: size(ctx.buttons("size", &["small", "medium", "large"], 1)),
        steppers: ctx.toggle("steppers", true),
        readonly: ctx.toggle("readonly", false),
        disabled: ctx.toggle("disabled", false),
        width: ctx.slider("width", 360.0, 220.0, 640.0, 1.0),
    };
    props.value = props.value.clamp(props.min, props.max);

    stage!(ctx, ui, |ui| {
        ui.set_width(props.width);
        input::show_number(
            ui,
            &mut props.value,
            input::NumberProps::new("Target distance (km)")
                .message(input::Message::Helper("Used to estimate the route length"))
                .size(props.size)
                .min(props.min)
                .max(props.max)
                .step(props.step)
                .steppers(props.steppers)
                .readonly(props.readonly)
                .disabled(props.disabled),
        );
    });
}

#[scene]
fn number_states(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        sample(
            ui,
            "Standard",
            Some(input::Message::Helper("Values from 0 to 100")),
            true,
            false,
            false,
        );
        ui.add_space(16.0);
        sample(
            ui,
            "Invalid",
            Some(input::Message::Error("Enter a value from 0 to 100")),
            true,
            false,
            false,
        );
        ui.add_space(16.0);
        sample(ui, "Without steppers", None, false, false, false);
        ui.add_space(16.0);
        sample(ui, "Read only", None, true, true, false);
        ui.add_space(16.0);
        sample(ui, "Disabled", None, true, false, true);
    });
}

#[scene]
fn number_sizes(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        for (label, size) in [
            ("Small", Size::Small),
            ("Medium", Size::Medium),
            ("Large", Size::Large),
        ] {
            let mut value = 50_i32;
            input::show_number(
                ui,
                &mut value,
                input::NumberProps::new(label).size(size).min(0).max(100),
            );
            ui.add_space(16.0);
        }
    });
}

fn sample(
    ui: &mut Ui,
    label: &str,
    message: Option<input::Message<'_>>,
    steppers: bool,
    readonly: bool,
    disabled: bool,
) {
    let mut value = 50_i32;
    input::show_number(
        ui,
        &mut value,
        input::NumberProps::new(label)
            .min(0)
            .max(100)
            .steppers(steppers)
            .readonly(readonly)
            .disabled(disabled)
            .message(message),
    );
}

const fn size(index: usize) -> Size {
    match index {
        0 => Size::Small,
        2 => Size::Large,
        _ => Size::Medium,
    }
}
