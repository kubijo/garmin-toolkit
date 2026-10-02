use gallery::prelude::*;
use garmin_cli_tui::{RunProfile, preview::PreviewScreen};

scene_meta! { title: "TUI / Operations / Removal" }

#[scene(default, order = 10)]
fn confirmation(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::RemovalConfirmation,
    );
}

#[scene(order = 20)]
fn backup(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::RemovalBackup,
    );
}

#[scene(order = 30)]
fn commit(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::RemovalCommit,
    );
}

#[scene(order = 40)]
fn complete(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::RemovalComplete,
    );
}

#[scene(order = 50)]
fn recovery_complete(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::RemovalRecoveryComplete,
    );
}
