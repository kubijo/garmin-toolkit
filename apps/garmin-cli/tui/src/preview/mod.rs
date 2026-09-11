//! Gallery fixtures and interaction state; all views use production renderers.

mod errors;
mod progress;
mod screens;
mod state;

use crate::{
    LOADING_SPINNER_INTERVAL, LoadingActivity, LoadingScreen, RunProfile, draw_profile_banner,
    profile_content_area, selected_formatter,
};
use garmin_i18n::format_message;
use garmin_progress::OperationStage;
pub use state::PreviewState;

/// Stable visual states exposed to the development gallery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewScreen {
    DeviceSelection,
    NoDevices,
    PendingRecovery,
    ContactGarmin,
    LoadingComponents,
    ReadingStorage,
    StorageCapacity,
    StorageUnavailable,
    StorageStress,
    StorageRefreshFailed,
    MapSelection,
    UpdateConfirmation(bool),
    RemovalConfirmation,
    PipelineProbeConfirmation,
    PipelineProbeComplete,
    LinkBenchmark,
    LinkBenchmarkComplete,
    LinkBenchmarkFinalize,
    LinkBenchmarkRecovery,
    VerificationComplete,
    UpdateStage(OperationStage),
    ConcurrentProgress,
    ProgressOverflow,
    RemovalBackup,
    RemovalCommit,
    RemovalComplete,
    UpdateRecoveryComplete,
    RemovalRecoveryComplete,
    AbortConfirmation,
    Completion,
    ErrorDownloadConnection,
    ErrorDownloadRejected,
    ErrorDownloadSize,
    ErrorDownloadChecksum,
    ErrorGarminResponse,
    ErrorGarminService,
    ErrorDeviceUpload,
    ErrorDeviceVerification,
    ErrorDeviceCleanup,
    ErrorMountedDevice,
    ErrorRemovalChanged,
    ErrorRemovalRollback,
    ErrorRemovalRecovery,
    ErrorUnexpected,
    ErrorStorageCapacity,
    ErrorRecoveryEvidence,
}

/// Render one gallery fixture through the same functions used by the terminal application.
pub fn render_preview(
    frame: &mut ratatui::Frame<'_>,
    profile: RunProfile,
    screen: PreviewScreen,
    animation_frame: usize,
    preview: &mut PreviewState,
) {
    preview.prepare(screen);
    let area = profile_content_area(frame.area(), profile);
    match screen {
        PreviewScreen::DeviceSelection => screens::render_device_preview(frame, area, false),
        PreviewScreen::NoDevices => screens::render_device_preview(frame, area, true),
        PreviewScreen::PendingRecovery => screens::render_pending_recovery_preview(frame),
        PreviewScreen::ContactGarmin => screens::render_contact_garmin_preview(frame),
        PreviewScreen::LoadingComponents | PreviewScreen::ReadingStorage => {
            render_loading_preview(frame, area, animation_frame, screen);
        }
        PreviewScreen::StorageCapacity
        | PreviewScreen::StorageUnavailable
        | PreviewScreen::StorageStress => {
            screens::render_storage_preview(
                frame,
                area,
                screen == PreviewScreen::StorageUnavailable,
                screen == PreviewScreen::StorageStress,
            );
        }
        PreviewScreen::StorageRefreshFailed => {
            screens::render_storage_refresh_failure_preview(frame, area);
        }
        PreviewScreen::MapSelection => screens::render_storage_preview(frame, area, false, false),
        PreviewScreen::UpdateConfirmation(backup_enabled) => {
            screens::render_confirmation_preview(frame, backup_enabled);
        }
        PreviewScreen::RemovalConfirmation => screens::render_removal_confirmation_preview(frame),
        PreviewScreen::PipelineProbeConfirmation => {
            screens::render_pipeline_probe_confirmation_preview(frame);
        }
        PreviewScreen::PipelineProbeComplete => {
            progress::render_pipeline_complete_preview(frame, area, preview);
        }
        PreviewScreen::LinkBenchmark
        | PreviewScreen::LinkBenchmarkComplete
        | PreviewScreen::LinkBenchmarkFinalize
        | PreviewScreen::LinkBenchmarkRecovery => {
            render_link_benchmark_preview(frame, area, screen, preview);
        }
        PreviewScreen::VerificationComplete => {
            progress::render_verification_complete_preview(frame, area, preview);
        }
        PreviewScreen::UpdateStage(stage) => {
            progress::render_stage(frame, area, stage, preview);
        }
        PreviewScreen::ConcurrentProgress | PreviewScreen::ProgressOverflow => {
            progress::render_concurrent(
                frame,
                area,
                animation_frame,
                screen == PreviewScreen::ProgressOverflow,
                preview,
            );
        }
        PreviewScreen::RemovalBackup => {
            progress::render_removal(frame, area, OperationStage::Backup, None, preview);
        }
        PreviewScreen::RemovalCommit => {
            progress::render_removal(frame, area, OperationStage::Commit, None, preview);
        }
        PreviewScreen::RemovalComplete => render_removal_complete(frame, area, preview),
        PreviewScreen::UpdateRecoveryComplete => {
            progress::render_update_recovery_preview(frame, area, preview);
        }
        PreviewScreen::RemovalRecoveryComplete => {
            progress::render_removal_recovery_preview(frame, area, preview);
        }
        PreviewScreen::AbortConfirmation => screens::render_abort_preview(frame),
        PreviewScreen::Completion => {
            progress::render_completion_preview(frame, area, preview);
        }
        PreviewScreen::ErrorDownloadConnection
        | PreviewScreen::ErrorDownloadRejected
        | PreviewScreen::ErrorDownloadSize
        | PreviewScreen::ErrorDownloadChecksum
        | PreviewScreen::ErrorGarminResponse
        | PreviewScreen::ErrorGarminService
        | PreviewScreen::ErrorDeviceUpload
        | PreviewScreen::ErrorDeviceVerification
        | PreviewScreen::ErrorDeviceCleanup
        | PreviewScreen::ErrorMountedDevice
        | PreviewScreen::ErrorRemovalChanged
        | PreviewScreen::ErrorRemovalRollback
        | PreviewScreen::ErrorRemovalRecovery
        | PreviewScreen::ErrorUnexpected
        | PreviewScreen::ErrorStorageCapacity
        | PreviewScreen::ErrorRecoveryEvidence => errors::render_failure_preview(frame, screen),
    }
    draw_profile_banner(frame, profile);
}

fn render_removal_complete(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    preview: &mut PreviewState,
) {
    let intl = selected_formatter();
    let completion = format_message!(
        &intl,
        default_message: "Removal complete. Verified backups retained."
    );
    progress::render_removal(
        frame,
        area,
        OperationStage::Cleanup,
        Some(&completion),
        preview,
    );
}

fn render_loading_preview(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    animation_frame: usize,
    screen: PreviewScreen,
) {
    let activity = if screen == PreviewScreen::ReadingStorage {
        LoadingActivity::DeviceStorage
    } else {
        LoadingActivity::MapComponents
    };
    let elapsed =
        LOADING_SPINNER_INTERVAL.saturating_mul(u32::try_from(animation_frame).unwrap_or(u32::MAX));
    LoadingScreen::new(activity, animation_frame, elapsed).render(frame, area);
}

fn render_link_benchmark_preview(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    screen: PreviewScreen,
    preview: &mut PreviewState,
) {
    match screen {
        PreviewScreen::LinkBenchmark => {
            progress::render_link_benchmark_preview(frame, area, preview);
        }
        PreviewScreen::LinkBenchmarkComplete => {
            progress::render_link_benchmark_complete_preview(frame, area, preview);
        }
        PreviewScreen::LinkBenchmarkFinalize | PreviewScreen::LinkBenchmarkRecovery => {
            progress::render_link_benchmark_finalize_preview(
                frame,
                area,
                screen == PreviewScreen::LinkBenchmarkRecovery,
                preview,
            );
        }
        _ => {}
    }
}
