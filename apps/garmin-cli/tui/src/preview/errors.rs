use super::PreviewScreen;
use crate::{
    FailureBody, FailureField, FailureValueTone, draw_failure_body, failure_dialog_config,
    render_dialog_backdrop,
};
use ratatui_interact::components::{DialogState, PopupDialog};

pub(super) fn render_failure_preview(frame: &mut ratatui::Frame<'_>, screen: PreviewScreen) {
    let (title, summary, fields) = match screen {
        PreviewScreen::ErrorStorageCapacity | PreviewScreen::ErrorRecoveryEvidence => {
            storage_failure_content(screen)
        }
        PreviewScreen::ErrorDownloadConnection => (
            "Download server unavailable",
            "Could not reach a Garmin download host.",
            vec![
                FailureField::new("Server", "omtmpaupdate.garmin.cn", FailureValueTone::Url),
                FailureField::new("Reason", "DNS lookup failed", FailureValueTone::Error),
            ],
        ),
        PreviewScreen::ErrorDownloadRejected => (
            "Download request rejected",
            "All Garmin download hosts rejected the file.",
            vec![FailureField::new(
                "Response",
                "403 Forbidden",
                FailureValueTone::Error,
            )],
        ),
        PreviewScreen::ErrorDownloadSize => (
            "Downloaded file is incomplete",
            "Downloaded size does not match the plan.",
            vec![
                FailureField::new("File", "Garmin/Mock/example.img", FailureValueTone::Path),
                FailureField::new("Expected", "12000000 bytes", FailureValueTone::Neutral),
                FailureField::new("Received", "10000000 bytes", FailureValueTone::Error),
            ],
        ),
        PreviewScreen::ErrorDownloadChecksum => (
            "Downloaded file is corrupt",
            "Downloaded file does not match Garmin's checksum.",
            vec![
                FailureField::new("File", "Garmin/Mock/example.img", FailureValueTone::Path),
                FailureField::new(
                    "Expected",
                    "11111111111111111111111111111111",
                    FailureValueTone::Neutral,
                ),
                FailureField::new(
                    "Received",
                    "22222222222222222222222222222222",
                    FailureValueTone::Error,
                ),
            ],
        ),
        PreviewScreen::ErrorGarminResponse => (
            "Invalid Garmin response",
            "Could not parse Garmin's update response.",
            vec![FailureField::new(
                "Parser",
                "missing field `maps` at line 1 column 274",
                FailureValueTone::Error,
            )],
        ),
        PreviewScreen::ErrorGarminService => (
            "Garmin service unavailable",
            "Could not reach Garmin's map-update service.",
            vec![FailureField::new(
                "Reason",
                "Connection timed out",
                FailureValueTone::Error,
            )],
        ),
        PreviewScreen::ErrorDeviceUpload
        | PreviewScreen::ErrorDeviceVerification
        | PreviewScreen::ErrorDeviceCleanup
        | PreviewScreen::ErrorMountedDevice => device_failure_preview_content(screen),
        PreviewScreen::ErrorRemovalChanged
        | PreviewScreen::ErrorRemovalRollback
        | PreviewScreen::ErrorRemovalRecovery => removal_failure_preview_content(screen),
        PreviewScreen::ErrorUnexpected => (
            "Update failed",
            "The update failed.",
            vec![FailureField::new(
                "Reason",
                "Unexpected update preparation failure",
                FailureValueTone::Error,
            )],
        ),
        _ => unreachable!("only failure previews use the failure renderer"),
    };
    let config = failure_dialog_config(title);
    let body = failure_preview_body(screen, summary, fields);
    let mut state = DialogState::new(body);
    state.register_button(0);
    state.show();
    render_dialog_backdrop(frame);
    PopupDialog::new(&config, &mut state, draw_failure_body).render(frame);
}

fn failure_preview_body(
    screen: PreviewScreen,
    summary: &'static str,
    fields: Vec<FailureField>,
) -> FailureBody {
    let pipeline_probe = is_pipeline_probe_failure(screen);
    let capture = if pipeline_probe {
        ".tmp/session7"
    } else if is_removal_failure(screen) {
        ".tmp/removal-run"
    } else {
        ".tmp/session2"
    };
    let body = FailureBody::new(summary, fields, capture);
    match screen {
        PreviewScreen::ErrorStorageCapacity => {
            body.with_outcome("Writing blocked. Check storage; keep this capture.")
        }
        PreviewScreen::ErrorRecoveryEvidence => {
            body.with_outcome("Files and backups preserved. Inspect before retrying.")
        }
        _ if pipeline_probe => {
            body.with_outcome("Pipeline probe incomplete; existing Garmin files were not changed.")
        }
        PreviewScreen::ErrorRemovalChanged => {
            body.with_outcome("Removal stopped before an unverified object could be deleted.")
        }
        PreviewScreen::ErrorRemovalRollback => {
            body.with_outcome("Keep this capture and run `device recover-removal` before retrying.")
        }
        PreviewScreen::ErrorRemovalRecovery => {
            body.with_outcome("No unverified file was written to the device.")
        }
        _ => body,
    }
}

fn storage_failure_content(
    screen: PreviewScreen,
) -> (&'static str, &'static str, Vec<FailureField>) {
    if screen == PreviewScreen::ErrorStorageCapacity {
        (
            "Storage check failed",
            "Storage cannot satisfy this operation.",
            vec![
                FailureField::new("Storage", "Internal storage", FailureValueTone::Neutral),
                FailureField::new("Required", "4.20 GB", FailureValueTone::Neutral),
                FailureField::new("Free", "1.30 GB", FailureValueTone::Error),
            ],
        )
    } else {
        (
            "Recovery evidence mismatch",
            "Recovery cannot safely identify this file.",
            vec![FailureField::new(
                "File",
                "Garmin/Maps/Regional-cycle-map.img",
                FailureValueTone::Path,
            )],
        )
    }
}

const fn is_removal_failure(screen: PreviewScreen) -> bool {
    matches!(
        screen,
        PreviewScreen::ErrorRemovalChanged
            | PreviewScreen::ErrorRemovalRollback
            | PreviewScreen::ErrorRemovalRecovery
    )
}

fn removal_failure_preview_content(
    screen: PreviewScreen,
) -> (&'static str, &'static str, Vec<FailureField>) {
    match screen {
        PreviewScreen::ErrorRemovalChanged => (
            "Device contents changed",
            "A removal target no longer matches the reviewed plan.",
            vec![
                FailureField::new("File", "Garmin/Mock/removal.img", FailureValueTone::Path),
                FailureField::new(
                    "State",
                    "RegularFile, size Some(12000000)",
                    FailureValueTone::Error,
                ),
            ],
        ),
        PreviewScreen::ErrorRemovalRollback => (
            "Removal recovery required",
            "Automatic rollback could not restore every selected file.",
            vec![
                FailureField::new(
                    "Removal",
                    "desktop MTP connection closed",
                    FailureValueTone::Error,
                ),
                FailureField::new(
                    "Rollback",
                    "Garmin/Mock/example.sid could not be restored",
                    FailureValueTone::Error,
                ),
            ],
        ),
        PreviewScreen::ErrorRemovalRecovery => (
            "Removal recovery failed",
            "The interrupted removal could not be recovered.",
            vec![FailureField::new(
                "Reason",
                "the recovery journal belongs to a different device",
                FailureValueTone::Error,
            )],
        ),
        _ => unreachable!("only removal failures use the removal failure preview"),
    }
}

const fn is_pipeline_probe_failure(screen: PreviewScreen) -> bool {
    matches!(
        screen,
        PreviewScreen::ErrorDeviceUpload
            | PreviewScreen::ErrorDeviceVerification
            | PreviewScreen::ErrorDeviceCleanup
            | PreviewScreen::ErrorMountedDevice
    )
}

fn device_failure_preview_content(
    screen: PreviewScreen,
) -> (&'static str, &'static str, Vec<FailureField>) {
    match screen {
        PreviewScreen::ErrorDeviceUpload => (
            "Device upload failed",
            "Could not copy the verified file through the desktop MTP mount.",
            vec![FailureField::new(
                "Reason",
                "Operation not supported",
                FailureValueTone::Error,
            )],
        ),
        PreviewScreen::ErrorDeviceVerification => (
            "Device verification failed",
            "The disposable device copy has an unexpected size.",
            vec![
                FailureField::new("Expected", "12000000 bytes", FailureValueTone::Neutral),
                FailureField::new("Received", "10000000 bytes", FailureValueTone::Error),
            ],
        ),
        PreviewScreen::ErrorDeviceCleanup => (
            "Device cleanup failed",
            "A failed upload left a disposable object that could not be removed.",
            vec![
                FailureField::new("Upload", "Connection closed", FailureValueTone::Error),
                FailureField::new("Cleanup", "Device is busy", FailureValueTone::Error),
            ],
        ),
        PreviewScreen::ErrorMountedDevice => (
            "Mounted device operation failed",
            "Could not complete the probe through the desktop MTP mount.",
            vec![FailureField::new(
                "Reason",
                "The mounted device is no longer available",
                FailureValueTone::Error,
            )],
        ),
        _ => unreachable!("only device failures use the device failure preview"),
    }
}
