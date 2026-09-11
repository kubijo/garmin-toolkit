use gallery::prelude::*;
use garmin_ui::device;

scene_meta! { title: "Components / Device state / Collection" }

#[scene(default)]
fn connecting(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show(
        ctx,
        ui,
        device::CollectionState::Connecting("Connecting to the device host…"),
        520.0,
    );
}

#[scene]
fn empty(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show(
        ctx,
        ui,
        device::CollectionState::Empty("No Garmin devices are connected."),
        520.0,
    );
}

#[scene]
fn failure(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show(
        ctx,
        ui,
        device::CollectionState::Error(
            "Could not reach the device host. Reconnect, then reload Garmin Toolkit.",
        ),
        360.0,
    );
}

fn show(ctx: &mut SceneCtx<'_>, ui: &mut Ui, state: device::CollectionState<'_>, width: f32) {
    garmin_ui::theme::apply(ui.style_mut());
    ctx.stage(ui, Stage::Fixed(egui::vec2(width, 220.0)), |ui| {
        ui.heading("Garmin Toolkit");
        ui.label("Devices");
        ui.add_space(16.0);
        device::show_collection_state(ui, state);
    });
}
