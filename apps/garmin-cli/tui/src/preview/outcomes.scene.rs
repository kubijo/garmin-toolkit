use gallery::prelude::*;
use garmin_cli_tui::{RunProfile, preview::PreviewScreen};

scene_meta! { title: "TUI / Outcomes" }

#[scene(order = 10)]
fn abort_confirmation(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::AbortConfirmation,
    );
}

#[scene(default, order = 20)]
fn completion(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(ctx, ui, RunProfile::PRODUCTION, PreviewScreen::Completion);
}
