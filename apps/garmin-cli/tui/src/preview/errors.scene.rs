use gallery::prelude::*;
use garmin_cli_tui::{RunProfile, preview::PreviewScreen};

scene_meta! { title: "TUI / Failures" }

#[scene(order = 5)]
fn storage_check_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorStorageCapacity,
    );
}

#[scene(order = 6)]
fn recovery_evidence_mismatch(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorRecoveryEvidence,
    );
}

#[scene(default, order = 10)]
fn download_server_unavailable(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::REAL_DRY_RUN,
        PreviewScreen::ErrorDownloadConnection,
    );
}

#[scene(order = 20)]
fn download_request_rejected(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorDownloadRejected,
    );
}

#[scene(order = 30)]
fn downloaded_file_incomplete(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorDownloadSize,
    );
}

#[scene(order = 40)]
fn downloaded_file_corrupt(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorDownloadChecksum,
    );
}

#[scene(order = 50)]
fn invalid_garmin_response(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorGarminResponse,
    );
}

#[scene(order = 60)]
fn garmin_service_unavailable(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorGarminService,
    );
}

#[scene(order = 70)]
fn device_upload_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorDeviceUpload,
    );
}

#[scene(order = 80)]
fn device_verification_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorDeviceVerification,
    );
}

#[scene(order = 90)]
fn device_cleanup_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorDeviceCleanup,
    );
}

#[scene(order = 100)]
fn mounted_device_operation_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorMountedDevice,
    );
}

#[scene(order = 105)]
fn removal_target_changed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorRemovalChanged,
    );
}

#[scene(order = 106)]
fn removal_rollback_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorRemovalRollback,
    );
}

#[scene(order = 107)]
fn removal_recovery_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ErrorRemovalRecovery,
    );
}

#[scene(order = 110)]
fn unexpected_failure(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    crate::screens::show_terminal(ctx, ui, RunProfile::MOCK, PreviewScreen::ErrorUnexpected);
}
