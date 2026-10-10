//! Deployment-wide backup and restore presentation.

use std::time::Duration;

use crate::{Size, button, icons, modal, progress};
use cint::ColorInterop;
use egui::{Id, RichText, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_service_api::snapshots::{SnapshotPreview, SnapshotState, SnapshotStatus};

pub mod recovery;

#[derive(Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Starting,
    Running(SnapshotStatus),
    Saved,
    DownloadStarted,
    Restored(RestoreSummary),
    Cancelled,
    Failed(String),
}

impl State {
    #[must_use]
    pub fn busy(&self) -> bool {
        matches!(self, Self::Starting | Self::Running(_))
    }

    #[must_use]
    pub fn switching(&self) -> bool {
        matches!(self, Self::Running(status) if status.state == SnapshotState::Restoring)
    }
}

/// Verified facts retained after a completed restore.
#[derive(Clone, Debug, Default)]
pub struct RestoreSummary {
    /// Time spent applying the approved restore, excluding the approval wait.
    pub elapsed: Option<Duration>,
    /// Size of the compressed backup file.
    pub archive_bytes: Option<u64>,
    /// Size of the verified database inside the backup.
    pub database_bytes: Option<u64>,
    /// Creation time from the verified backup manifest.
    pub created_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Backup,
    Restore,
    ServerBackup,
    ServerRestore,
    Approve,
    Cancel,
    Clear,
}

#[derive(Clone, Debug)]
pub enum SelectedFile {
    SaveDestination(String),
    RestoreSource(String),
}

pub struct Props<'a> {
    pub intl: &'a Intl,
    pub state: &'a State,
    pub file: Option<&'a SelectedFile>,
    pub enabled: bool,
    pub server_files: bool,
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    let inline_padding = if ui.available_width() < 480.0 { 16 } else { 24 };
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: inline_padding,
            right: inline_padding,
            top: 24,
            bottom: 24,
        })
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(640.0));
            ui.heading(format_message!(intl, default_message: "Backup and restore"));
            let description = format_message!(intl, default_message: "A backup includes all profiles, activities, original files, routes, and pictures. Device pairing and cloud credentials are excluded.");
            crate::typography::body(ui, &description);
        });
    if props.file.is_none() && !props.state.busy() && props.enabled {
        let action = if props.server_files {
            show_locations(ui, props)
        } else {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(inline_padding, 0))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(640.0));
                    show_picker(ui, props)
                })
                .inner
        };
        if !matches!(props.state, State::Idle) {
            ui.add_space(16.0);
            return egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(inline_padding, 0))
                .show(ui, |ui| show_operation(ui, props))
                .inner
                .or(action);
        }
        return action;
    }
    if props.file.is_some() {
        show_operation(ui, props)
    } else {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(inline_padding, 0))
            .show(ui, |ui| show_operation(ui, props))
            .inner
    }
}

fn show_picker(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    let save = format_message!(intl, default_message: "Save backup");
    let open = format_message!(intl, default_message: "Open backup");
    let buttons = [
        (
            "backup.save",
            Action::Backup,
            button::Props {
                label: &save,
                icon: Some(icons::FLOPPY_DISK),
                kind: button::Kind::Primary,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: props.enabled && !props.state.busy(),
            },
        ),
        (
            "backup.open",
            Action::Restore,
            button::Props {
                label: &open,
                icon: Some(icons::FOLDER_OPEN),
                kind: button::Kind::Secondary,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: props.enabled && !props.state.busy(),
            },
        ),
    ];
    action_row(ui, &buttons)
}

fn show_locations(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    let inline_padding = if ui.available_width() < 480.0 { 16 } else { 24 };
    let download = format_message!(intl, default_message: "Download to this device");
    let save = format_message!(intl, default_message: "Save on server");
    let upload = format_message!(intl, default_message: "Upload from this device");
    let open = format_message!(intl, default_message: "Open from server");
    let mut action = None;
    for (index, (title, description, pictogram, buttons)) in [
        (
            format_message!(intl, default_message: "Create a backup"),
            format_message!(intl, default_message: "Save a copy of the current application data."),
            icons::FLOPPY_DISK,
            [
                (
                    "backup.save",
                    Action::Backup,
                    &download,
                    icons::DOWNLOAD_SIMPLE,
                ),
                (
                    "backup.server-save",
                    Action::ServerBackup,
                    &save,
                    icons::HARD_DRIVE,
                ),
            ],
        ),
        (
            format_message!(intl, default_message: "Restore a backup"),
            format_message!(intl, default_message: "Choose an existing backup. Review it before replacing any data."),
            icons::CLOCK_COUNTER_CLOCKWISE,
            [
                (
                    "backup.open",
                    Action::Restore,
                    &upload,
                    icons::UPLOAD_SIMPLE,
                ),
                (
                    "backup.server-open",
                    Action::ServerRestore,
                    &open,
                    icons::FOLDER_OPEN,
                ),
            ],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            ui.add_space(8.0);
        }
        let selected = location_card(
            ui,
            &title,
            &description,
            pictogram,
            inline_padding,
            &buttons.map(|(id, requested, label, icon)| {
                (
                    id,
                    requested,
                    button::Props {
                        label,
                        icon: Some(icon),
                        kind: button::Kind::Secondary,
                        size: Size::Medium,
                        width: button::Width::Fit,
                        enabled: props.enabled,
                    },
                )
            }),
        );
        action = selected.or(action);
    }
    action
}

fn location_card(
    ui: &mut Ui,
    title: &str,
    description: &str,
    pictogram: icons::Icon,
    inline_padding: i8,
    buttons: &[(&str, Action, button::Props<'_>)],
) -> Option<Action> {
    let palette = crate::theme::palette(ui);
    egui::Frame::NONE
        .fill(crate::theme::color32(
            palette.surfaces().layer(garmin_color::theme::Level::One),
        ))
        .inner_margin(egui::Margin::symmetric(inline_padding, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.scope(|ui| {
                ui.set_max_width(ui.available_width().min(640.0));
                ui.spacing_mut().item_spacing = egui::vec2(16.0, 8.0);
                let compact = buttons
                    .iter()
                    .map(|(_, _, button)| button.natural_width(ui))
                    .sum::<f32>()
                    + 8.0
                    + 64.0
                    + 16.0
                    > ui.available_width();
                let mut action = None;
                ui.horizontal(|ui| {
                    icons::Props {
                        icon: pictogram,
                        size: 64.0,
                        color: palette.content().text_primary(),
                    }
                    .show(ui);
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        ui.heading(title);
                        crate::typography::body(ui, description);
                        if !compact {
                            ui.add_space(8.0);
                            action = action_row(ui, buttons);
                        }
                    });
                });
                if compact {
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.x = 8.0;
                    action = action_row(ui, buttons);
                }
                action
            })
            .inner
        })
        .inner
}

fn action_row(ui: &mut Ui, buttons: &[(&str, Action, button::Props<'_>)]) -> Option<Action> {
    let stacked = buttons
        .iter()
        .map(|(_, _, button)| button.natural_width(ui))
        .sum::<f32>()
        + ui.spacing().item_spacing.x
        > ui.available_width();
    let mut action = None;
    let actions = |ui: &mut Ui| {
        for &(id, selected, mut button) in buttons {
            if stacked {
                button.width = button::Width::Fill;
            }
            let response = button.show(ui);
            crate::semantics::target(ui, &response, id);
            if response.clicked() {
                action = Some(selected);
            }
        }
    };
    if stacked {
        ui.vertical(actions);
    } else {
        ui.horizontal(actions);
    }
    action
}

/// Operation status shared with the profile chooser after a database switch.
#[must_use]
pub fn show_operation(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    if matches!(props.state, State::Idle) && props.file.is_none() {
        return None;
    }
    if matches!(props.state, State::Restored(_)) {
        return show_restore_acknowledgement(ui, props);
    }
    ui.scope(|ui| operation(ui, props)).inner
}

fn show_restore_acknowledgement(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let title = format_message!(props.intl, default_message: "Backup restored");
    let description = format_message!(props.intl, default_message: "Choose a profile to continue.");
    let continue_label = format_message!(props.intl, default_message: "Continue");
    let source_label = format_message!(props.intl, default_message: "Restore source");
    let State::Restored(summary) = props.state else {
        unreachable!("restore acknowledgement requires a completed restore");
    };
    let output = modal::show(
        ui,
        Id::new("backup-restore-complete"),
        &modal::Props {
            title: &title,
            description: Some(&description),
            size: modal::Size::Small,
            presentation: modal::Presentation::Modal,
            cancel_label: None,
            cancel_disabled: None,
            backdrop_closes: Some(false),
            primary: Some(modal::Primary {
                label: &continue_label,
                icon: Some(icons::CHECK),
                kind: modal::PrimaryKind::Confirm,
                enabled: true,
            }),
        },
        |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            if let Some(SelectedFile::RestoreSource(name)) = props.file {
                ui.label(RichText::new(&source_label).weak());
                ui.add(egui::Label::new(RichText::new(name).strong()).wrap());
            }
            show_restore_summary(ui, props.intl, summary);
        },
    );
    if let Some(primary) = &output.primary {
        crate::semantics::target(ui, primary, "backup.restore.acknowledge");
    }
    (output.action == Some(modal::Action::Primary)).then_some(Action::Clear)
}

fn show_restore_summary(ui: &mut Ui, intl: &Intl, summary: &RestoreSummary) {
    let mut facts = Vec::new();
    if let Some(elapsed) = summary.elapsed {
        let duration = if elapsed.as_secs() == 0 {
            format_message!(intl, default_message: "Less than a second")
        } else {
            crate::offline::format_duration(elapsed.as_secs())
        };
        facts.push((
            format_message!(intl, default_message: "Restore time"),
            duration,
        ));
    }
    if let Some(bytes) = summary.archive_bytes {
        facts.push((
            format_message!(intl, default_message: "Backup size"),
            crate::text::format_bytes(bytes),
        ));
    }
    if let Some(bytes) = summary.database_bytes {
        facts.push((
            format_message!(intl, default_message: "Database size"),
            crate::text::format_bytes(bytes),
        ));
    }
    if let Some(created_at) = summary.created_at {
        let created = garmin_model::value::Timestamp::from_unix_seconds(created_at);
        let date = created.map_or_else(
            |_| created_at.to_string(),
            |time| {
                intl.dates()
                    .datetime(time.as_jiff().to_zoned(jiff::tz::TimeZone::UTC).datetime())
            },
        );
        facts.push((
            format_message!(intl, default_message: "Backup created (UTC)"),
            date,
        ));
    }
    if !facts.is_empty() {
        ui.add_space(24.0);
        crate::facts::show(ui, "backup-restored-facts", &facts);
    }
}

fn operation(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    if let Some(file) = props.file {
        let (label, name) = match file {
            SelectedFile::SaveDestination(name) => (
                format_message!(intl, default_message: "Save destination"),
                name,
            ),
            SelectedFile::RestoreSource(name) => (
                format_message!(intl, default_message: "Restore source"),
                name,
            ),
        };
        let palette = crate::theme::palette(ui);
        let inline_padding = if ui.available_width() < 480.0 { 16 } else { 24 };
        return egui::Frame::NONE
            .fill(crate::theme::color32(
                palette.surfaces().layer(garmin_color::theme::Level::One),
            ))
            .inner_margin(egui::Margin::symmetric(inline_padding, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.scope(|ui| {
                    ui.set_max_width(ui.available_width().min(640.0));
                    if matches!(props.state, State::Saved) {
                        return show_saved(ui, intl, &label, name);
                    }
                    ui.label(
                        RichText::new(label).color(palette.content().text_secondary().into_cint()),
                    );
                    ui.add(egui::Label::new(RichText::new(name).strong()).wrap());
                    ui.add_space(16.0);
                    show_operation_content(ui, props)
                })
                .inner
            })
            .inner;
    }
    ui.set_max_width(ui.available_width().min(640.0));
    show_operation_content(ui, props)
}

fn show_saved(ui: &mut Ui, intl: &Intl, destination: &str, name: &str) -> Option<Action> {
    let palette = crate::theme::palette(ui);
    let secondary = crate::theme::color32(palette.content().text_secondary());
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
    ui.horizontal(|ui| {
        icons::Props {
            icon: icons::CHECK,
            size: 24.0,
            color: palette.support().success(),
        }
        .show(ui);
        ui.heading(format_message!(intl, default_message: "Backup saved"));
    });
    ui.add_space(8.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(RichText::new(destination).color(secondary));
        let (directory, filename) =
            name.split_at(name.rfind(['/', '\\']).map_or(0, |index| index + 1));
        let mut path = egui::text::LayoutJob::default();
        for text in [
            RichText::new(directory).color(secondary),
            crate::typography::semibold(filename),
        ] {
            text.append_to(
                &mut path,
                ui.style(),
                egui::FontSelection::Default,
                egui::Align::Center,
            );
        }
        ui.add(egui::Label::new(path).wrap());
    });
    ui.add_space(8.0);
    let done = button::Props {
        label: &format_message!(intl, default_message: "Done"),
        icon: None,
        kind: button::Kind::Primary,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled: true,
    }
    .show(ui);
    crate::semantics::target(ui, &done, "backup.clear");
    done.clicked().then_some(Action::Clear)
}

fn show_operation_content(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    let mut action = None;
    match props.state {
        State::Idle | State::Restored(_) => {}
        State::Starting => {
            progress::show(
                ui,
                &progress::Props {
                    height: Some(16.0),
                    label: &format_message!(intl, default_message: "Preparing backup operation"),
                    detail: None,
                    value: progress::Value::Indeterminate,
                },
            );
            if cancel(ui, intl) {
                action = Some(Action::Cancel);
            }
        }
        State::Running(status) => {
            action = show_status(ui, intl, status).or(action);
        }
        State::Saved => {
            ui.label(format_message!(intl, default_message: "Backup saved"));
        }
        State::DownloadStarted => {
            ui.label(format_message!(intl, default_message: "Backup download started. Check your browser’s downloads for completion."));
        }
        State::Cancelled => {
            ui.label(format_message!(intl, default_message: "Backup operation cancelled"));
        }
        State::Failed(error) => {
            crate::notification::show(
                ui,
                &crate::notification::Props {
                    kind: crate::notification::Kind::Error,
                    title: &format_message!(intl, default_message: "Backup operation failed"),
                    detail: Some(error),
                },
            );
        }
    }
    if props.file.is_some()
        && !props.state.busy()
        && (props.enabled || !matches!(props.state, State::Failed(_)))
    {
        ui.add_space(8.0);
        if control(
            ui,
            "backup.clear",
            &format_message!(intl, default_message: "Clear selection"),
            button::Kind::Tertiary,
            true,
        ) {
            action = Some(Action::Clear);
        }
    }
    action
}

fn show_preview(ui: &mut Ui, intl: &Intl, preview: &SnapshotPreview) {
    ui.heading(format_message!(intl, default_message: "Restore this backup?"));
    ui.add_space(8.0);
    let explanation = format_message!(intl, default_message: "This replaces all profiles and stored data in this application. Save a current backup first if you want to keep them.");
    crate::typography::body(ui, &explanation);
    ui.add_space(8.0);
    let created = garmin_model::value::Timestamp::from_unix_seconds(preview.created_at);
    let date = created.map_or_else(
        |_| preview.created_at.to_string(),
        |time| {
            intl.dates()
                .datetime(time.as_jiff().to_zoned(jiff::tz::TimeZone::UTC).datetime())
        },
    );
    let size = crate::text::format_bytes(preview.database_bytes);
    let facts = [
        (
            format_message!(intl, default_message: "Created (UTC)"),
            date.as_str(),
        ),
        (
            format_message!(intl, default_message: "App version"),
            preview.app_version.as_str(),
        ),
        (
            format_message!(intl, default_message: "Database size"),
            size.as_str(),
        ),
    ];
    crate::facts::show(ui, "backup-preview-facts", &facts);
    ui.add_space(16.0);
}

fn show_status(ui: &mut Ui, intl: &Intl, status: &SnapshotStatus) -> Option<Action> {
    if let Some(preview) = &status.preview {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            show_preview(ui, intl, preview);
        });
        let approve = format_message!(intl, default_message: "Replace all data");
        let cancel = format_message!(intl, default_message: "Cancel");
        let confirm_button = button::Props {
            label: &approve,
            icon: None,
            kind: button::Kind::Danger,
            size: Size::Medium,
            width: button::Width::Fit,
            enabled: true,
        };
        return action_row(
            ui,
            &[
                ("backup.approve", Action::Approve, confirm_button),
                (
                    "backup.cancel",
                    Action::Cancel,
                    button::Props {
                        label: &cancel,
                        kind: button::Kind::Tertiary,
                        ..confirm_button
                    },
                ),
            ],
        );
    }
    let label = match status.state {
        SnapshotState::Uploading => format_message!(intl, default_message: "Reading backup"),
        SnapshotState::Verifying | SnapshotState::AwaitingApproval => {
            format_message!(intl, default_message: "Verifying backup")
        }
        SnapshotState::Restoring => {
            format_message!(intl, default_message: "Restoring data. Please keep the application open.")
        }
        _ => format_message!(intl, default_message: "Saving backup"),
    };
    let value = match status.state {
        SnapshotState::Uploading | SnapshotState::DownloadReady => status
            .total
            .and_then(|total| {
                Some(progress::Value::Determinate {
                    completed: usize::try_from(status.transferred).ok()?,
                    total: usize::try_from(total).ok()?,
                })
            })
            .unwrap_or(progress::Value::Indeterminate),
        _ => progress::Value::Indeterminate,
    };
    progress::show(
        ui,
        &progress::Props {
            height: Some(16.0),
            label: &label,
            detail: None,
            value,
        },
    );
    (status.state != SnapshotState::Restoring && cancel(ui, intl)).then_some(Action::Cancel)
}

fn cancel(ui: &mut Ui, intl: &Intl) -> bool {
    control(
        ui,
        "backup.cancel",
        &format_message!(intl, default_message: "Cancel"),
        button::Kind::Tertiary,
        true,
    )
}

fn control(ui: &mut Ui, id: &str, label: &str, kind: button::Kind, enabled: bool) -> bool {
    let response = button::Props {
        label,
        icon: None,
        kind,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
    .show(ui);
    crate::semantics::target(ui, &response, id);
    response.clicked()
}
