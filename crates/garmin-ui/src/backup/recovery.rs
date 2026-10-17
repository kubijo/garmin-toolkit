//! Explicit recovery of host-owned work after a browser reload.

use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::snapshots::{SnapshotOperation, SnapshotSource, SnapshotState};

use crate::{Size, button, typography};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Refresh,
    Resume(usize),
    Discard(usize),
}

pub struct Props<'a> {
    pub intl: &'a Intl,
    pub operations: &'a [SnapshotOperation],
    pub loading: bool,
    pub error: Option<&'a str>,
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    let mut action = None;
    ui.set_max_width(ui.available_width().min(640.0));
    ui.heading(format_message!(intl, default_message: "Previous backup operations"));
    if let Some(error) = props.error {
        crate::notification::show(
            ui,
            &crate::notification::Props {
                kind: crate::notification::Kind::Error,
                title: &format_message!(intl, default_message: "Could not recover backup operations"),
                detail: Some(error),
            },
        );
    }
    if props.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format_message!(intl, default_message: "Checking previous operations…"));
        });
    }
    for (index, operation) in props.operations.iter().enumerate() {
        ui.push_id(operation.status.operation, |ui| {
            let palette = crate::theme::palette(ui);
            egui::Frame::new()
                .fill(crate::theme::color32(
                    palette.surfaces().layer(garmin_color::theme::Level::One),
                ))
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    ui.label(typography::semibold(source_label(intl, &operation.source)));
                    typography::body(ui, &detail(intl, operation));
                    let id = operation.status.operation;
                    ui.horizontal_wrapped(|ui| {
                        if resumable(operation)
                            && control(
                                ui,
                                &format!("resume.{id}"),
                                &format_message!(intl, default_message: "Resume"),
                                !props.loading && !operation.active,
                            )
                        {
                            action = Some(Action::Resume(index));
                        }
                        let label = if terminal(operation.status.state) {
                            format_message!(intl, default_message: "Dismiss")
                        } else {
                            format_message!(intl, default_message: "Discard")
                        };
                        if control(
                            ui,
                            &format!("discard.{id}"),
                            &label,
                            !props.loading
                                && !operation.active
                                && operation.status.state != SnapshotState::Restoring,
                        ) {
                            action = Some(Action::Discard(index));
                        }
                    });
                });
        });
        ui.add_space(8.0);
    }
    if control(
        ui,
        "refresh",
        &format_message!(intl, default_message: "Refresh"),
        !props.loading,
    ) {
        action = Some(Action::Refresh);
    }
    action
}

#[must_use]
pub fn source_label(intl: &Intl, source: &SnapshotSource) -> String {
    match source {
        SnapshotSource::Download => format_message!(intl, default_message: "Backup download"),
        SnapshotSource::Upload => format_message!(intl, default_message: "Uploaded backup"),
        SnapshotSource::ServerSave(path) | SnapshotSource::ServerRestore(path) => path.clone(),
    }
}

fn resumable(operation: &SnapshotOperation) -> bool {
    !(terminal(operation.status.state)
        || operation.source == SnapshotSource::Upload
            && operation.status.state == SnapshotState::Uploading)
}

fn terminal(state: SnapshotState) -> bool {
    matches!(
        state,
        SnapshotState::Completed | SnapshotState::Cancelled | SnapshotState::Failed
    )
}

fn detail(intl: &Intl, operation: &SnapshotOperation) -> String {
    if operation.active {
        return format_message!(intl, default_message: "In use in another window or download. Refresh after it closes.");
    }
    match operation.status.state {
        SnapshotState::Uploading if operation.source == SnapshotSource::Upload => {
            format_message!(intl, default_message: "Upload interrupted. Discard it, then select the backup file again.")
        }
        SnapshotState::Uploading => format_message!(intl, default_message: "Reading backup"),
        SnapshotState::Verifying => format_message!(intl, default_message: "Verifying backup"),
        SnapshotState::AwaitingApproval => {
            format_message!(intl, default_message: "Ready to review. Resuming does not replace any data.")
        }
        SnapshotState::BackingUp => format_message!(intl, default_message: "Saving backup"),
        SnapshotState::DownloadReady => {
            format_message!(intl, default_message: "Backup ready to download")
        }
        SnapshotState::Restoring => {
            format_message!(intl, default_message: "Restoring data. Please keep the application open.")
        }
        SnapshotState::Completed => match operation.source {
            SnapshotSource::ServerSave(_) | SnapshotSource::Download => {
                format_message!(intl, default_message: "Backup saved")
            }
            _ => {
                format_message!(intl, default_message: "Backup restored. Choose a profile to continue.")
            }
        },
        SnapshotState::Cancelled => {
            format_message!(intl, default_message: "Backup operation cancelled")
        }
        SnapshotState::Failed => {
            operation.status.error.clone().unwrap_or_else(
                || format_message!(intl, default_message: "Backup operation failed"),
            )
        }
    }
}

fn control(ui: &mut Ui, id: &str, label: &str, enabled: bool) -> bool {
    let response = button::Props {
        label,
        icon: None,
        kind: button::Kind::Tertiary,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
    .show(ui);
    crate::semantics::target(ui, &response, format!("backup.recovery.{id}"));
    response.clicked()
}
