use gallery::prelude::*;
use garmin_cli_tui::{RunProfile, preview::PreviewScreen};
use garmin_progress::OperationStage;

scene_meta! { title: "TUI / Operations / Update" }

#[scene(default, order = 10)]
fn backup(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Backup),
    );
}

#[scene(order = 20)]
fn download(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Download),
    );
}

#[scene(order = 21)]
fn concurrent_files(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ConcurrentProgress,
    );
}

#[scene(order = 22)]
fn active_files_overflow(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ProgressOverflow,
    );
}

#[scene(order = 30)]
fn verify(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Verify),
    );
}

#[scene(order = 40)]
fn authorize(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Authorize),
    );
}

#[scene(order = 50)]
fn stage(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Stage),
    );
}

#[scene(order = 60)]
fn commit(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Commit),
    );
}

#[scene(order = 70)]
fn cleanup(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateStage(OperationStage::Cleanup),
    );
}

#[scene(order = 80)]
fn pipeline_complete(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::PipelineProbeComplete,
    );
}

#[scene(order = 90)]
fn link_benchmark(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::LinkBenchmark,
    );
}

#[scene(order = 91)]
fn link_benchmark_complete(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::LinkBenchmarkComplete,
    );
}

#[scene(order = 92)]
fn link_benchmark_finalize(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::LinkBenchmarkFinalize,
    );
}

#[scene(order = 93)]
fn link_benchmark_recovery(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::LinkBenchmarkRecovery,
    );
}

#[scene(order = 110)]
fn update_recovery_complete(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateRecoveryComplete,
    );
}
