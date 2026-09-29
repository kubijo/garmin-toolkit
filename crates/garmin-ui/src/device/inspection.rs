use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_model::device::{DeviceInspection, InspectionSection, TransactionKind};

pub(super) fn show(ui: &mut Ui, intl: &Intl, report: &DeviceInspection) {
    if let InspectionSection::Unavailable(error) = &report.manifest {
        failure(
            ui,
            intl,
            &format_message!(intl, default_message: "Device manifest"),
            error,
        );
    }
    if let InspectionSection::Unavailable(error) = &report.storage {
        failure(
            ui,
            intl,
            &format_message!(intl, default_message: "Storage"),
            error,
        );
    }
    for storage in &report.toolkit {
        ui.push_id(&storage.storage_id, |ui| {
            ui.separator();
            let label = match &report.storage {
                InspectionSection::Available(state) => state.storages.iter()
                    .find(|item| item.id == storage.storage_id).map(|item| item.label.as_str()),
                _ => None,
            }.unwrap_or(&storage.storage_id);
            if report.toolkit.len() > 1 { ui.strong(label); }
            section(ui, intl, &format_message!(intl, default_message: "Toolkit state"), &storage.namespace, |ui, id| {
                details(ui, intl, |ui| { ui.label(id.to_string()); });
            });
            section(ui, intl, &format_message!(intl, default_message: "Pairing"), &storage.identity, |ui, identity| {
                ui.label(if !identity.verified {
                    format_message!(intl, default_message: "Device identity not verified")
                } else if identity.paired_user_id.is_some() {
                    format_message!(intl, default_message: "Paired user recorded")
                } else {
                    format_message!(intl, default_message: "Not paired")
                });
                details(ui, intl, |ui| {
                    super::metadata(ui, &format_message!(intl, default_message: "Device ID"), &identity.device_id.to_string());
                    if let Some(id) = identity.paired_user_id {
                        super::metadata(ui, &format_message!(intl, default_message: "Recorded user ID"), &id.to_string());
                    }
                });
            });
            section(ui, intl, &format_message!(intl, default_message: "Transaction"), &storage.transaction, |ui, transaction| {
                ui.label(if transaction.completed {
                    format_message!(intl, default_message: "Completed; cleanup pending")
                } else {
                    format_message!(intl, default_message: "Pending transaction")
                });
                details(ui, intl, |ui| {
                    ui.label(transaction.transaction_id.to_string());
                    ui.label(match transaction.kind {
                        TransactionKind::Update => format_message!(intl, default_message: "Update"),
                        TransactionKind::Removal => format_message!(intl, default_message: "Removal"),
                    });
                    if !transaction.verified {
                        ui.label(format_message!(intl, default_message: "Device identity not verified"));
                    }
                });
            });
        });
    }
    ui.add_space(8.0);
}

fn section<T>(
    ui: &mut Ui,
    intl: &Intl,
    label: &str,
    value: &InspectionSection<T>,
    available: impl FnOnce(&mut Ui, &T),
) {
    ui.push_id(label, |ui| match value {
        InspectionSection::Missing => {
            ui.horizontal_wrapped(|ui| {
                ui.label(label);
                ui.label(format_message!(intl, default_message: "Not present"));
            });
        }
        InspectionSection::Available(value) => {
            ui.label(label);
            available(ui, value);
        }
        InspectionSection::Unavailable(error) => failure(ui, intl, label, error),
    });
}

fn failure(ui: &mut Ui, intl: &Intl, label: &str, error: &garmin_model::device::InspectionFailure) {
    egui::CollapsingHeader::new(format_message!(intl, default_message: "{section}: unavailable", values: { section: label }))
        .id_salt(label)
        .show(ui, |ui| {
            super::inspection_error(
                ui,
                &format_message!(intl, default_message: "Inspection error"),
                &error.message,
            );
        })
        .header_response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
}

fn details(ui: &mut Ui, intl: &Intl, contents: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(format_message!(intl, default_message: "Details"))
        .show(ui, contents)
        .header_response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
}
