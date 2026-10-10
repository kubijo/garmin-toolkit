use gallery::prelude::*;
use garmin_cli_tui::{RunProfile, preview::PreviewScreen};
use garmin_progress::OperationStage;

scene_meta! { title: "TUI / Setup / Execution profiles" }

#[scene(default, order = 10)]
fn dry_run_confirmation(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::REAL_DRY_RUN,
        PreviewScreen::UpdateConfirmation(true),
    );
}

#[scene(order = 20)]
fn dry_run_download(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::REAL_DRY_RUN,
        PreviewScreen::UpdateStage(OperationStage::Download),
    );
}

#[scene(order = 30)]
fn dry_run_authorize(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::REAL_DRY_RUN,
        PreviewScreen::UpdateStage(OperationStage::Authorize),
    );
}

#[scene(order = 40)]
fn dry_run_completion(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::REAL_DRY_RUN,
        PreviewScreen::VerificationComplete,
    );
}

#[scene(order = 50)]
fn mock_confirmation(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::MOCK,
        PreviewScreen::UpdateConfirmation(true),
    );
}
