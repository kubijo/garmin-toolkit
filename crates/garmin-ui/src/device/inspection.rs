use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_model::device::{
    DeviceInspection, InspectionSection, StorageInspection, TransactionKind,
};

use crate::facts;

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
            let label = match &report.storage {
                InspectionSection::Available(state) => state
                    .storages
                    .iter()
                    .find(|item| item.id == storage.storage_id)
                    .map(|item| item.label.as_str()),
                _ => None,
            }
            .unwrap_or(&storage.storage_id);
            if report.toolkit.len() > 1 {
                ui.strong(label);
            }
            ui.add_space(8.0);
            storage_summary(ui, intl, storage);
            storage_details(ui, intl, storage);
            ui.add_space(8.0);
        });
    }
    ui.add_space(8.0);
}

fn storage_summary(ui: &mut Ui, intl: &Intl, storage: &StorageInspection) {
    let toolkit = format_message!(intl, default_message: "Toolkit state");
    let pairing = format_message!(intl, default_message: "Pairing");
    let transaction = format_message!(intl, default_message: "Transaction");
    let namespace_status = status(
        intl,
        &storage.namespace,
        |_| format_message!(intl, default_message: "Present"),
    );
    let pairing_status = status(intl, &storage.identity, |identity| {
        if !identity.verified {
            format_message!(intl, default_message: "Device identity not verified")
        } else if identity.paired_user_id.is_some() {
            format_message!(intl, default_message: "Paired user recorded")
        } else {
            format_message!(intl, default_message: "Not paired")
        }
    });
    let transaction_status = status(intl, &storage.transaction, |transaction| {
        if transaction.completed {
            format_message!(intl, default_message: "Completed; cleanup pending")
        } else {
            format_message!(intl, default_message: "Pending transaction")
        }
    });
    facts::show(
        ui,
        "toolkit-summary",
        &[
            (&toolkit, &namespace_status),
            (&pairing, &pairing_status),
            (&transaction, &transaction_status),
        ],
    );
    for (label, error) in [
        (&toolkit, unavailable(&storage.namespace)),
        (&pairing, unavailable(&storage.identity)),
        (&transaction, unavailable(&storage.transaction)),
    ] {
        if let Some(error) = error {
            failure(ui, intl, label, error);
        }
    }
}

fn status<T>(
    intl: &Intl,
    value: &InspectionSection<T>,
    available: impl FnOnce(&T) -> String,
) -> String {
    match value {
        InspectionSection::Missing => format_message!(intl, default_message: "Not present"),
        InspectionSection::Available(value) => available(value),
        InspectionSection::Unavailable(_) => format_message!(intl, default_message: "Unavailable"),
    }
}

fn unavailable<T>(
    value: &InspectionSection<T>,
) -> Option<&garmin_model::device::InspectionFailure> {
    match value {
        InspectionSection::Unavailable(error) => Some(error),
        _ => None,
    }
}

fn storage_details(ui: &mut Ui, intl: &Intl, storage: &StorageInspection) {
    let mut rows = Vec::new();
    if let InspectionSection::Available(id) = &storage.namespace {
        rows.push((
            format_message!(intl, default_message: "Toolkit ID"),
            id.to_string(),
        ));
    }
    if let InspectionSection::Available(identity) = &storage.identity {
        rows.push((
            format_message!(intl, default_message: "Device ID"),
            identity.device_id.to_string(),
        ));
        if let Some(id) = identity.paired_user_id {
            rows.push((
                format_message!(intl, default_message: "Recorded user ID"),
                id.to_string(),
            ));
        }
    }
    if let InspectionSection::Available(transaction) = &storage.transaction {
        rows.push((
            format_message!(intl, default_message: "Transaction ID"),
            transaction.transaction_id.to_string(),
        ));
        rows.push((
            format_message!(intl, default_message: "Operation"),
            match transaction.kind {
                TransactionKind::Update => format_message!(intl, default_message: "Update"),
                TransactionKind::Removal => format_message!(intl, default_message: "Removal"),
            },
        ));
    }
    if rows.is_empty() {
        return;
    }
    for (label, _) in &mut rows {
        *label = label.replace(' ', "\u{a0}");
    }
    ui.add_space(8.0);
    facts::show(ui, "toolkit-details", &rows);
    if let InspectionSection::Available(transaction) = &storage.transaction
        && !transaction.verified
    {
        ui.add_space(8.0);
        ui.label(format_message!(intl, default_message: "Device identity not verified"));
    }
}

fn failure(ui: &mut Ui, intl: &Intl, label: &str, error: &garmin_model::device::InspectionFailure) {
    crate::accordion::show(
        ui,
        &crate::accordion::Props {
            id: &format!("device.inspection.failure.{label}"),
            label: &format_message!(intl, default_message: "{section}: unavailable", values: { section: label }),
            default_open: false,
            inline_padding: 16,
        },
        |ui| {
            super::inspection_error(
                ui,
                &format_message!(intl, default_message: "Inspection error"),
                &error.message,
            );
        },
    );
}
