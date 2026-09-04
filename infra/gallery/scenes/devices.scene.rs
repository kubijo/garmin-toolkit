use gallery::prelude::*;
use garmin_cli_tui::{RunProfile, preview::PreviewScreen};

scene_meta! { title: "TUI / Devices" }

#[scene(default, order = 10)]
fn device_selection(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::DeviceSelection,
    );
}

#[scene(order = 20)]
fn no_devices(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(ctx, ui, RunProfile::PRODUCTION, PreviewScreen::NoDevices);
}
