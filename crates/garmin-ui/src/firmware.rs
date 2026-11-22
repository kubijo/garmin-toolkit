//! Device software selection, approval and transfer status shared by both graphical clients.

use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_model::artifact::ByteCount;
use garmin_service_api::firmware::{
    Action, Component, InstalledStatus, Observation, Phase, ReceiptStatus, RetainedTransfer,
    Review, State, TransferProgress,
};

use crate::action_row;
use crate::action_row::Placement;
use crate::notification::{Kind, Toast, ToastEvent, ToastId, Toasts};
use crate::{Size, button, checkbox, icons, typography};
use garmin_service_api::{DeviceSnapshot, firmware::AvailableUpdate};
use std::cmp::Ordering;
use std::collections::HashMap;
mod transcript;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Select,
    Review,
    Write,
    Install,
}

fn steps(ui: &mut Ui, intl: &Intl, current: Step) {
    use crate::progress_indicator;
    use crate::progress_indicator::{Props, Status};
    let labels = [
        format_message!(intl, default_message: "Select updates"),
        format_message!(intl, default_message: "Review"),
        format_message!(intl, default_message: "Write"),
        format_message!(intl, default_message: "Install"),
        format_message!(intl, default_message: "Verify"),
    ];
    let active = current as usize;
    let stages = std::array::from_fn::<_, 5, _>(|index| progress_indicator::Step {
        label: &labels[index],
        description: None,
        status: match index.cmp(&active) {
            Ordering::Less => Status::Complete,
            Ordering::Equal => Status::Current,
            Ordering::Greater => Status::Upcoming,
        },
    });
    let _ = progress_indicator::show(
        ui,
        intl,
        &Props {
            steps: &stages,
            interactive: false,
        },
    );
    ui.add_space(16.0);
}

pub(crate) fn update_title(intl: &Intl, name: &str, update: &AvailableUpdate) -> String {
    format_message!(intl, default_message: "Firmware {version} is available for {device}", values: {
        version: update.version.as_str(), device: name,
    })
}

/// Keep the notification dismissed across redraws, manual checks and browser reconnects.
#[derive(Default)]
pub struct Notices {
    seen: HashMap<String, (AvailableUpdate, Option<ToastId>)>,
}

impl Notices {
    pub fn sync(
        &mut self,
        intl: &Intl,
        devices: &[DeviceSnapshot],
        toasts: &mut Toasts,
        workspace_open: bool,
    ) {
        self.seen.retain(|key, (previous, id)| {
            let device = devices.iter().find(|device| &device.key == key);
            let current = device.and_then(|d| d.firmware_update.as_ref()) == Some(previous);
            if !current && let Some(id) = id.take() {
                toasts.dismiss(id);
            }
            current
        });
        if !workspace_open {
            return;
        }
        for device in devices {
            let Some(update) = &device.firmware_update else {
                continue;
            };
            if self
                .seen
                .get(&device.key)
                .is_some_and(|(seen, _)| seen == update)
            {
                continue;
            }
            let title = update_title(intl, &device.name, update);
            let toast = Toast::new(Kind::Information, title)
                .action(format_message!(intl, default_message: "Device updates"));
            let id = toasts.push(toast);
            self.seen
                .insert(device.key.clone(), (update.clone(), Some(id)));
        }
    }

    pub fn handle(&mut self, event: ToastEvent) -> Option<String> {
        let (ToastEvent::Invoked(id) | ToastEvent::Dismissed(id)) = event;
        let (key, (_, pending)) = self
            .seen
            .iter_mut()
            .find(|(_, (_, pending))| *pending == Some(id))?;
        *pending = None;
        matches!(event, ToastEvent::Invoked(_)).then(|| key.clone())
    }
}

pub fn show(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    egui::ScrollArea::vertical()
        .id_salt("device-updates")
        .show(ui, |ui| {
            let inset = if ui.available_width() < 480.0 { 16 } else { 32 };
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(inset, 24))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(880.0));
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.heading(format_message!(intl, default_message: "Device updates"));
                    ui.add_space(16.0);
                    body(ui, intl, state)
                })
                .inner
        })
        .inner
}

fn body(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let phase = &state.phase;
    let background = state.inspection.as_ref().map_or(phase, |i| &i.background);
    if let Phase::WriteFailed(reason) | Phase::Failed(reason) = background {
        let title = if matches!(background, Phase::WriteFailed(_)) {
            format_message!(intl, default_message: "Update transfer stopped")
        } else {
            format_message!(intl, default_message: "Couldn’t check for updates")
        };
        failure_notice(ui, intl, &title, None, reason);
        if matches!(background, Phase::WriteFailed(_)) {
            transcript::show(ui, intl, &state.transcript, true);
        }
        ui.add_space(16.0);
    }
    if let Some(inspection) = &state.inspection {
        return ui
            .push_id("previous-transfer", |ui| {
                receipt(ui, intl, phase, inspection.from_list)
            })
            .inner;
    }
    if matches!(phase, Phase::PreviousTransferPending)
        || state.has_retained_transfer
            && matches!(
                phase,
                Phase::Available(_) | Phase::Consent | Phase::WriteFailed(_) | Phase::Failed(_)
            )
    {
        return load_receipt(ui, intl, state);
    }
    match phase {
        Phase::Disconnected => {
            ui.label(format_message!(intl, default_message: "The selected device is no longer connected"));
            None
        }
        Phase::WriteFailed(_) | Phase::Failed(_) => control(
            ui,
            &format_message!(intl, default_message: "Check for updates"),
            icons::ARROWS_CLOCKWISE,
            true,
            true,
        )
        .then_some(Action::Check),
        Phase::Consent => {
            ui.label(if state.simulated {
                format_message!(intl, default_message: "Check the connected device against the local firmware simulator.")
            } else {
                format_message!(intl, default_message: "Checking sends this device’s identity and installed software versions to Garmin.")
            });
            ui.add_space(16.0);
            control(
                ui,
                &format_message!(intl, default_message: "Check for updates"),
                icons::ARROWS_CLOCKWISE,
                true,
                true,
            )
            .then_some(Action::Check)
        }
        Phase::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format_message!(intl, default_message: "Checking device updates…"));
            });
            None
        }
        Phase::Available(components) => catalog(ui, intl, state, components),
        Phase::Staging(progress) => {
            steps(ui, intl, Step::Write);
            if state.transcript.stages.is_empty() {
                transfer_progress(ui, intl, progress);
            } else {
                transcript::show(ui, intl, &state.transcript, false);
            }
            None
        }
        Phase::ReadyToEject { needs_restart } => {
            steps(ui, intl, Step::Install);
            ui.label(typography::semibold(
                format_message!(intl, default_message: "Ready to eject"),
            ));
            ui.label(format_message!(intl, default_message: "Eject and disconnect the device so it can apply the updates. Reconnect it to check the installed versions."));
            if *needs_restart {
                ui.label(
                    format_message!(intl, default_message: "The update requires a device restart."),
                );
            }
            transcript::show(ui, intl, &state.transcript, true);
            None
        }
        _ => None,
    }
}

fn load_receipt(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    ui.label(format_message!(intl, default_message: "Inspecting…"));
    // A remote host can redraw the same snapshot before it acknowledges the request.
    let key = ui.id().with("requested-receipt");
    if ui.is_enabled() && ui.data(|d| d.get_temp(key)) != Some(state.revision) {
        ui.data_mut(|d| d.insert_temp(key, state.revision));
        return Some(Action::Inspect);
    }
    control(
        ui,
        &format_message!(intl, default_message: "Retry"),
        icons::ARROWS_CLOCKWISE,
        false,
        ui.is_enabled(),
    )
    .then_some(Action::Inspect)
}

fn failure_notice(ui: &mut Ui, intl: &Intl, title: &str, detail: Option<&str>, reason: &str) {
    crate::notification::show(
        ui,
        &crate::notification::Props {
            kind: Kind::Error,
            title,
            detail,
        },
    );
    crate::accordion::inline(
        ui,
        "firmware.failure_details",
        &format_message!(intl, default_message: "Technical details"),
        |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(reason).monospace())
                    .wrap()
                    .selectable(true),
            );
        },
    );
}

fn transfer_progress(ui: &mut Ui, intl: &Intl, progress: &TransferProgress) {
    ui.label(typography::semibold(
        format_message!(intl, default_message: "Staging updates"),
    ));
    ui.label(&progress.label);
    if let (Some(done), Some(total)) = (progress.completed, progress.total) {
        crate::progress::show(
            ui,
            &crate::progress::Props {
                label: &format!("{} / {}", done.format_binary(), total.format_binary()),
                detail: None,
                height: None,
                value: crate::progress::Value::Determinate {
                    completed: usize::from(done.ratio_scaled(total, 1000)),
                    total: 1000,
                },
            },
        );
    } else {
        ui.spinner();
    }
}

fn transfer_list(ui: &mut Ui, intl: &Intl, transfers: &[RetainedTransfer]) -> Option<Action> {
    ui.spacing_mut().item_spacing.y = 24.0;
    ui.label(format_message!(intl, default_message: "Choose a receipt to see the details of a transfer to this device."));
    for transfer in transfers {
        let (status, explanation) = match transfer.status {
            ReceiptStatus::VersionsUnverified => (
                format_message!(intl, default_message: "Installed versions unverified"),
                format_message!(intl, default_message: "Transfer completed. This older receipt cannot verify installed versions."),
            ),
            ReceiptStatus::RecoveryRequired => (
                format_message!(intl, default_message: "Recovery required"),
                format_message!(intl, default_message: "The transfer was interrupted. Open the receipt to review recovery options."),
            ),
            ReceiptStatus::Unavailable => (
                format_message!(intl, default_message: "Receipt unavailable"),
                format_message!(intl, default_message: "Open the receipt to see why its details could not be verified."),
            ),
            ReceiptStatus::Confirmed => (
                format_message!(intl, default_message: "Update confirmed"),
                format_message!(intl, default_message: "The device reports the expected version for every selected component."),
            ),
            ReceiptStatus::AwaitingConfirmation => (
                format_message!(intl, default_message: "Awaiting device confirmation"),
                format_message!(intl, default_message: "Open the receipt to see which component versions the device has reported so far."),
            ),
        };
        let local = transfer
            .registered_at
            .as_jiff()
            .to_zoned(jiff::tz::TimeZone::system())
            .datetime();
        let date = intl.dates().datetime(local);
        let id = transfer.receipt.as_simple().to_string();
        let clicked = ui
            .push_id(transfer.receipt, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(typography::semibold(status));
                typography::helper(ui, &explanation);
                control(
                    ui,
                    &format!("{date} · {}", &id[..8]),
                    icons::FILE,
                    false,
                    true,
                )
            })
            .inner;
        if clicked {
            return Some(Action::InspectReceipt(transfer.receipt));
        }
    }
    None
}

fn receipt(ui: &mut Ui, intl: &Intl, phase: &Phase, from_list: bool) -> Option<Action> {
    ui.label(typography::semibold(
        format_message!(intl, default_message: "Previous transfer"),
    ));
    ui.add_space(8.0);
    let selected = receipt_body(ui, intl, phase);
    let primary = match phase {
        Phase::Reconnected(_) | Phase::VersionsUnverified => Some((
            format_message!(intl, default_message: "Dismiss transfer receipt"),
            icons::CHECK,
            Action::Acknowledge,
        )),
        Phase::RecoveryRequired => Some((
            format_message!(intl, default_message: "Recover interrupted transfer"),
            icons::TOOLBOX,
            Action::Recover,
        )),
        Phase::ReviewRecovery { approval, .. } => Some((
            format_message!(intl, default_message: "Preserve and restore files"),
            icons::TOOLBOX,
            Action::ApproveRecovery(*approval),
        )),
        Phase::Failed(_) if !from_list => Some((
            format_message!(intl, default_message: "Retry"),
            icons::ARROWS_CLOCKWISE,
            Action::Inspect,
        )),
        Phase::NoTransfer => Some((
            format_message!(intl, default_message: "Continue"),
            icons::CARET_RIGHT,
            Action::CloseInspection,
        )),
        _ => None,
    };
    let back = format_message!(intl, default_message: "Back");
    let back_action = (
        "firmware.receipts",
        Action::Inspect,
        control_props(&back, icons::CARET_LEFT, false, true),
    );
    let can_go_back = matches!(phase, Phase::ReviewRecovery { .. })
        || from_list
            && !matches!(
                phase,
                Phase::Transfers(_) | Phase::Checking | Phase::Recovering
            );
    ui.add_space(16.0);
    if let Some((label, icon, action)) = primary {
        let next = (
            "firmware.receipt_action",
            action,
            control_props(&label, icon, true, true),
        );
        if can_go_back {
            action_row::show(ui, Placement::Trailing, [next, back_action])
        } else {
            action_row::show(ui, Placement::Trailing, [next])
        }
    } else if can_go_back {
        action_row::show(ui, Placement::Leading, [back_action])
    } else {
        selected
    }
}

fn receipt_body(ui: &mut Ui, intl: &Intl, phase: &Phase) -> Option<Action> {
    ui.spacing_mut().item_spacing.y = 8.0;
    match phase {
        Phase::Reconnected(items) => {
            receipt_components(ui, intl, items);
            ui.add_space(8.0);
            let explanation = if items
                .iter()
                .any(|item| matches!(item.status, InstalledStatus::OtherVersion(_)))
            {
                format_message!(intl, default_message: "The device reports a different version for at least one component. You can dismiss this receipt and check for available updates.")
            } else if items
                .iter()
                .all(|item| item.status == InstalledStatus::Confirmed)
                && !items.is_empty()
            {
                format_message!(intl, default_message: "The device reports the expected version for every selected component.")
            } else {
                format_message!(intl, default_message: "If installation is pending, eject the device, follow any installation prompt, then reconnect to verify. To retry an update, dismiss the receipt and select it again.")
            };
            typography::body(ui, &explanation);
        }
        Phase::Transfers(items) => return transfer_list(ui, intl, items),
        Phase::ReviewRecovery { files, .. } => {
            ui.label(format_message!(intl, default_message: "Keep a copy of these empty uploads and restore the original files?"));
            for file in files {
                ui.label(file);
            }
        }
        Phase::Failed(reason) => {
            failure_notice(
                ui,
                intl,
                &format_message!(intl, default_message: "Couldn’t complete the receipt action"),
                None,
                reason,
            );
        }
        Phase::NoTransfer => {
            ui.label(format_message!(intl, default_message: "No firmware transfer was found for this device."));
        }
        Phase::RecoveryRequired => {
            ui.label(format_message!(intl, default_message: "The transfer was interrupted. Reconnect the same device to restore its files."));
        }
        Phase::VersionsUnverified => {
            ui.label(format_message!(intl, default_message: "Transfer completed. This older receipt cannot verify installed versions."));
        }
        Phase::Checking | Phase::Recovering => {
            ui.horizontal(|ui| {
                ui.spinner();
                let message = if matches!(phase, Phase::Recovering) {
                    format_message!(intl, default_message: "Recovering interrupted transfer…")
                } else {
                    format_message!(intl, default_message: "Inspecting…")
                };
                ui.label(message);
            });
        }
        _ => {}
    }
    None
}

fn receipt_components(ui: &mut Ui, intl: &Intl, observations: &[Observation]) {
    use crate::summary_table;
    use crate::summary_table::{Cell, Column};
    use egui::Align;
    let name = format_message!(intl, default_message: "Selected updates");
    let result = format_message!(intl, default_message: "Installed versions after reconnect");
    summary_table::show(ui, &name, [Column::new(&name, 1, Align::Min), Column::new(&result, 1, Align::Min)], observations.iter().map(|item| {
        let status = match &item.status {
            InstalledStatus::Confirmed => format_message!(intl, default_message: "Update confirmed"),
            InstalledStatus::Unchanged => format_message!(intl, default_message: "Installed version unchanged"),
            InstalledStatus::Unreported => format_message!(intl, default_message: "Absent from device inventory"),
            InstalledStatus::OtherVersion(version) => format_message!(intl, default_message: "Device reports {version}", values: {version: version.as_str()}),
        };
        [Cell {text: typography::semibold(format!("{} · {}", item.name, item.offered)), detail: Some(item.part_number.clone()), fraction: None}, Cell::new(status)]
    }), None);
}

fn catalog(ui: &mut Ui, intl: &Intl, state: &State, components: &[Component]) -> Option<Action> {
    if components.is_empty() {
        ui.label(
            format_message!(intl, default_message: "No updates were offered for this device."),
        );
        return control(
            ui,
            &format_message!(intl, default_message: "Check for updates"),
            icons::ARROWS_CLOCKWISE,
            true,
            true,
        )
        .then_some(Action::Check);
    }
    let key = ui.id().with(state.catalog_id);
    if let Some(review) = &state.review {
        ui.data_mut(|data| data.insert_temp(key, review.selected.clone()));
        return review_selection(ui, intl, components, review);
    }
    steps(ui, intl, Step::Select);
    let mut selection = ui
        .data(|data| data.get_temp::<Vec<u32>>(key))
        .unwrap_or_default();
    for (index, item) in components.iter().enumerate() {
        let index = u32::try_from(index).expect("bounded catalog");
        let mut checked = selection.contains(&index);
        ui.push_id(index, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_enabled_ui(item.download_bytes.is_some() && item.staging_issue.is_none(), |ui| {
                checkbox::show(ui, &mut checked, typography::semibold(&item.name));
            });
            ui.spacing_mut().indent = checkbox::LABEL_INSET;
            ui.indent("detail", |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                component_versions(ui, intl, item);
                if let Some(issue) = &item.staging_issue {
                    use garmin_model::firmware::StagingIssue;
                    ui.label(match issue {
                        StagingIssue::InstallationInstructions => format_message!(intl, default_message: "Additional installation instructions are unsupported."),
                        StagingIssue::DeliveryRestriction(channel) => format_message!(intl, default_message: "Unsupported delivery restriction: {channel}", values: {channel: channel.as_str()}),
                    });
                }
                if item.has_metadata { ui.weak(format_message!(intl, default_message: "Includes update metadata")); }
                if let Some(description) = &item.description && description != &item.name && !description.is_empty() {
                    ui.add(egui::Label::new(description).selectable(false));
                }
                if !item.release_notes.is_empty() {
                    let id = format!("firmware.release-notes.{}", item.part_number);
                    crate::accordion::inline(ui, &id, &format_message!(intl, default_message: "Release notes"), |ui| {
                        for note in &item.release_notes {
                            ui.strong(&note.version);
                            for change in &note.changes { ui.label(change); }
                        }
                    });
                }
            });
        });
        if checked && !selection.contains(&index) {
            selection.push(index);
        }
        if !checked {
            selection.retain(|&part| part != index);
        }
        ui.add_space(8.0);
    }
    selection.sort_unstable();
    ui.data_mut(|data| data.insert_temp(key, selection.clone()));
    ui.separator();
    ui.add_space(8.0);
    let review = format_message!(intl, default_message: "Review selection");
    let button = control_props(&review, icons::EYE, true, !selection.is_empty());
    action_row::show(
        ui,
        Placement::Trailing,
        [("firmware.review", Action::Review(selection), button)],
    )
}

fn review_selection(
    ui: &mut Ui,
    intl: &Intl,
    components: &[Component],
    review: &Review,
) -> Option<Action> {
    use crate::summary_table;
    use crate::summary_table::{Cell, Column};
    use egui::Align;

    steps(ui, intl, Step::Review);
    let items = review
        .selected
        .iter()
        .filter_map(|&index| components.get(index as usize));
    let name = format_message!(intl, default_message: "Selected updates");
    let size = format_message!(intl, default_message: "Download size");
    let total_label = format_message!(intl, default_message: "Total download");
    let total_size = review.download_bytes.format_binary();
    let total = (total_label.as_str(), total_size.as_str());
    let size_col = Column::new(&size, 2, Align::Max);
    if ui.available_width() >= 640.0 {
        let installed = format_message!(intl, default_message: "Installed");
        let available = format_message!(intl, default_message: "Available");
        let columns = [
            Column::new(&name, 5, Align::Min),
            Column::new(&installed, 2, Align::Max),
            Column::new(&available, 2, Align::Max),
            size_col,
        ];
        summary_table::show(
            ui,
            &name,
            columns,
            items.map(|item| {
                [
                    Cell::new(typography::semibold(&item.name)),
                    Cell::new(item.installed.as_deref().unwrap_or("—")),
                    Cell::new(item.available.as_str()),
                    Cell::new(typography::semibold(component_size(intl, item))),
                ]
            }),
            Some(total),
        );
    } else {
        let columns = [Column::new(&name, 4, Align::Min), size_col];
        summary_table::show(
            ui,
            &name,
            columns,
            items.map(|item| {
                [
                    Cell {
                        text: typography::semibold(&item.name),
                        detail: Some(version_text(intl, item)),
                        fraction: None,
                    },
                    Cell::new(typography::semibold(component_size(intl, item))),
                ]
            }),
            Some(total),
        );
    }
    ui.add_space(16.0);
    typography::body(
        ui,
        &format_message!(intl, default_message: "After writing, eject and disconnect the device to install the updates."),
    );
    ui.add_space(16.0);
    let write = format_message!(intl, default_message: "Write selected updates");
    let edit = format_message!(intl, default_message: "Edit selection");
    action_row::show(
        ui,
        Placement::Trailing,
        [
            (
                "firmware.stage",
                Action::Stage(review.approval),
                control_props(&write, icons::SEND_TO_DEVICE, true, true),
            ),
            (
                "firmware.edit",
                Action::EditSelection,
                control_props(&edit, icons::PENCIL, false, true),
            ),
        ],
    )
}

fn component_size(intl: &Intl, item: &Component) -> String {
    item.download_bytes.map_or_else(
        || format_message!(intl, default_message: "Download unavailable"),
        ByteCount::format_binary,
    )
}

fn version_text(intl: &Intl, item: &Component) -> String {
    if let Some(installed) = &item.installed {
        format_message!(intl, default_message: "Installed {installed} · Available {available}", values: {installed: installed.as_str(), available: item.available.as_str()})
    } else {
        format_message!(intl, default_message: "Available {available}", values: {available: item.available.as_str()})
    }
}

fn component_versions(ui: &mut Ui, intl: &Intl, item: &Component) {
    let size = component_size(intl, item);
    let versions = version_text(intl, item);
    ui.add(
        size_label(
            ui,
            &format!("{versions} · {size}"),
            item.download_bytes.map(|_| size.as_str()),
        )
        .selectable(false),
    );
}

fn size_label(ui: &Ui, text: &str, size: Option<&str>) -> egui::Label {
    let body = egui::TextStyle::Body.resolve(ui.style());
    egui::Label::new(typography::with_emphasis(
        text,
        size,
        body.size,
        ui.visuals().text_color(),
    ))
}

fn control(ui: &mut Ui, label: &str, icon: icons::Icon, primary: bool, enabled: bool) -> bool {
    control_props(label, icon, primary, enabled)
        .show(ui)
        .clicked()
}

fn control_props(
    label: &str,
    icon: icons::Icon,
    primary: bool,
    enabled: bool,
) -> button::Props<'_> {
    button::Props {
        label,
        icon: Some(icon),
        kind: if primary {
            button::Kind::Primary
        } else {
            button::Kind::Secondary
        },
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_i18n::{Language, Translations};
    use garmin_service_api::InspectionState;
    use uuid::Uuid;

    #[test]
    fn notification_opens_the_device_once_per_attachment() {
        let intl = Translations::bundled()
            .unwrap()
            .formatter(Language::English)
            .unwrap();
        let mut notices = Notices::default();
        let mut toasts = Toasts::default();
        let mut devices = [DeviceSnapshot {
            key: "watch".into(),
            name: "Watch".into(),
            identifier: None,
            software_version: None,
            firmware_update: None,
            inspection: InspectionState::Ready,
            inspection_error: None,
            report: None,
            capabilities: Vec::new(),
            storages: Vec::new(),
        }];
        notices.sync(&intl, &devices, &mut toasts, true);
        assert!(notices.seen.is_empty());
        devices[0].firmware_update = Some(AvailableUpdate {
            attachment: Uuid::from_u128(1),
            version: "24.20".into(),
        });
        notices.sync(&intl, &devices, &mut toasts, false);
        assert!(notices.seen.is_empty());
        notices.sync(&intl, &devices, &mut toasts, true);
        let first = notices.seen["watch"].1.unwrap();
        assert_eq!(
            notices.handle(ToastEvent::Invoked(first)).as_deref(),
            Some("watch")
        );
        notices.sync(&intl, &devices, &mut toasts, true);
        assert!(notices.seen["watch"].1.is_none());
        devices[0].firmware_update.as_mut().unwrap().attachment = Uuid::from_u128(2);
        notices.sync(&intl, &devices, &mut toasts, true);
        let second = notices.seen["watch"].1.unwrap();
        assert_ne!(first, second);
        notices.sync(&intl, &[], &mut toasts, true);
        assert_eq!(notices.handle(ToastEvent::Invoked(second)), None);
    }
}
