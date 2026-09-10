use garmin_device::{DeviceSummary, TransportKind};
use garmin_progress::{OperationStage, ProgressEventKind, ProgressState};
use indoc::indoc;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{ListState, TableState};
use ratatui_interact::components::{DialogState, PopupDialog};

use crate::{
    AbortBody, ConfirmationBody, MapActionMode, MapChoice, MapChoiceAction, MapVersionTone,
    OperationView, PIPELINE_PROBE_WRITE_WARNING, PendingRecoveryActions, SelectedMapAction,
    abort_dialog_config, confirmation_dialog_config, draw_abort_body, draw_confirmation_body,
    draw_map_selection, draw_update_confirmation_body, garmin_contact_confirmation_body,
    pending_recovery_dialog_config, removal_confirmation_body, render_abort_footer,
    render_confirmation_footer, render_device_selection, render_dialog_backdrop,
    render_pending_recovery_footer, render_update_confirmation_footer, update_confirmation_body,
    update_device_rows,
};

const PREVIEW_MAP_SERVICE_URL: &str = "https://omt.garmin.com/api/maps/universal/update";

pub(super) fn render_contact_garmin_preview(frame: &mut ratatui::Frame<'_>) {
    let config = confirmation_dialog_config("Contact Garmin?", "Contact Garmin");
    let mut state = DialogState::new(garmin_contact_confirmation_body(
        PREVIEW_MAP_SERVICE_URL.to_owned(),
        true,
    ));
    state.register_button(0);
    state.register_button(1);
    state.show();
    render_dialog_backdrop(frame);
    {
        let mut dialog = PopupDialog::new(&config, &mut state, draw_confirmation_body);
        dialog.render(frame);
    }
    render_confirmation_footer(frame);
}

pub(super) fn render_pending_recovery_preview(frame: &mut ratatui::Frame<'_>) {
    let actions = PendingRecoveryActions::RecoverOrClear;
    let config = pending_recovery_dialog_config(actions);
    let mut state = DialogState::new(ConfirmationBody {
        introduction: ratatui::text::Text::raw(indoc! {"
            An earlier operation for this device did not reach a completed operation report.
            Recover it before starting another device change."}),
        fields: vec![
            crate::ConfirmationField::new(
                indoc! {"Device"},
                indoc! {"
                    fenix 8 - 47mm, Solar (006-B4532-00)
                    desktop-mounted MTP at mounted-mtp:0e4054edc7e3f331"},
                crate::ConfirmationValueTone::Neutral,
            ),
            crate::ConfirmationField::new(
                "Operation",
                "Map update",
                crate::ConfirmationValueTone::Warning,
            ),
            crate::ConfirmationField::new(
                "Plan ID",
                "01953ce506d6d6da6c7bfafaf8eafd4e",
                crate::ConfirmationValueTone::Muted,
            ),
            crate::ConfirmationField::new(
                "Recovery",
                ".tmp/fenix-t03",
                crate::ConfirmationValueTone::Path,
            ),
        ],
        note: Some(ratatui::text::Text::raw(indoc! {"
            Recover reconciles the interrupted transaction.
            Clear state is allowed only after the device is proven fully updated or untouched."})),
        confirm_action: "recover now".to_owned(),
    });
    for index in 0..config.buttons.len() {
        state.register_button(index);
    }
    state.show();
    render_dialog_backdrop(frame);
    {
        let mut dialog = PopupDialog::new(&config, &mut state, draw_confirmation_body);
        dialog.render(frame);
    }
    render_pending_recovery_footer(frame, actions);
}

pub(super) fn render_device_preview(frame: &mut ratatui::Frame<'_>, area: Rect, empty: bool) {
    let rows = if empty {
        vec!["No Garmin device is currently visible".to_owned()]
    } else {
        update_device_rows(&[
            DeviceSummary {
                transport: TransportKind::MountedMtp,
                model: "Example Watch".to_owned(),
                part_number: Some("006-TEST-01".to_owned()),
                software_version: Some("99.01".to_owned()),
                location: "mounted-mtp:synthetic-watch".to_owned(),
            },
            DeviceSummary {
                transport: TransportKind::MountedMtp,
                model: "Example Cycling Computer".to_owned(),
                part_number: Some("006-TEST-02".to_owned()),
                software_version: Some("99.02".to_owned()),
                location: "mounted-mtp:synthetic-cycle".to_owned(),
            },
        ])
    };
    let mut state = ListState::default().with_selected((!empty).then_some(0));
    render_device_selection(
        frame,
        area,
        &rows,
        "Select an update device",
        true,
        &mut state,
    );
}

pub(super) fn render_map_preview(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let choices = [
        MapChoice {
            name: "Base maps".to_owned(),
            version_transition: "(9.00) → 9.00".to_owned(),
            version_tone: MapVersionTone::UpToDate,
            description: "Base maps · 2 files · 53.95 MB".to_owned(),
            install_label: "Reinstall",
            can_remove: false,
            cache: None,
        },
        MapChoice {
            name: "TopoActive Central Europe".to_owned(),
            version_transition: "(2024.10) → 2026.11".to_owned(),
            version_tone: MapVersionTone::Outdated,
            description: "Europe · TopoActive · 5 files · 5.30 GB".to_owned(),
            install_label: "Update",
            can_remove: true,
            cache: Some(super::super::MapCacheAvailability {
                cached_files: 5,
                total_files: 5,
            }),
        },
        MapChoice {
            name: "Garmin Ski Map".to_owned(),
            version_transition: "(unknown) → 2026.10".to_owned(),
            version_tone: MapVersionTone::Unknown,
            description: "Points of Interest · 2 files · 42.76 MB".to_owned(),
            install_label: "Install",
            can_remove: false,
            cache: Some(super::super::MapCacheAvailability {
                cached_files: 1,
                total_files: 2,
            }),
        },
    ];
    let actions = [
        MapChoiceAction::Keep,
        MapChoiceAction::Change,
        MapChoiceAction::Keep,
    ];
    let mut state = TableState::default().with_selected(Some(1));
    let areas = Layout::vertical([Constraint::Min(5), Constraint::Length(4)]).split(area);
    draw_map_selection(
        frame,
        (areas[0], areas[1]),
        &choices,
        &actions,
        &mut state,
        true,
        MapActionMode::Combined,
    );
}

pub(super) fn render_storage_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    unavailable: bool,
    stress: bool,
) {
    let mut state = storage_fixture(unavailable);
    if stress {
        "Internal storage exposed through a long desktop mount name"
            .clone_into(&mut state.storages[0].label);
        state.storages[0].capacity = garmin_device::StorageCapacity::unavailable(
            "Device disconnected. Reconnect, then refresh.",
        );
    }
    let area = crate::device_state::render(frame, area, Some(&state));
    render_map_preview(frame, area);
}

pub(super) fn render_storage_refresh_failure_preview(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let update = Err("The mounted device is no longer available".to_owned());
    let area = crate::device_state::render_update(frame, area, Some(&update));
    render_map_preview(frame, area);
}

pub(super) fn storage_fixture(unavailable: bool) -> garmin_device::DeviceStateSnapshot {
    use garmin_device::{DeviceStateSnapshot, DeviceStorageState, StorageCapacity};
    DeviceStateSnapshot {
        storages: vec![
            DeviceStorageState {
                id: "internal".to_owned(),
                label: "Internal storage".to_owned(),
                writable: Some(true),
                capacity: if unavailable {
                    StorageCapacity::unavailable("The mount did not report capacity")
                } else {
                    StorageCapacity::new(32_000_000_000, 8_600_000_000)
                },
            },
            DeviceStorageState {
                id: "card".to_owned(),
                label: "Memory card".to_owned(),
                writable: Some(false),
                capacity: StorageCapacity::new(64_000_000_000, 52_000_000_000),
            },
        ],
    }
}

pub(super) fn render_confirmation_preview(frame: &mut ratatui::Frame<'_>, backup_enabled: bool) {
    let body = update_confirmation_body(
        indoc! {"
            fenix 8 - 47mm, Solar (006-B4532-00)
            desktop-mounted MTP at mounted-mtp:0e4054edc7e3f331"},
        &[
            SelectedMapAction::new("Base maps", "Reinstall"),
            SelectedMapAction::new("Garmin Ski Map", "Install"),
            SelectedMapAction::new("TopoActive Central Europe", "Update"),
        ],
        "952bf20745678c8c8a39bb27016d99ca",
        "9 files",
        "6.40 GB",
        "9 files",
        ".tmp/fenix-update-20260906-02",
    )
    .with_backup_plan_ids(
        "952bf20745678c8c8a39bb27016d99ca",
        "59f49870b6577cc75e7d3f21b18e9bcb",
    )
    .with_backup_enabled(backup_enabled);
    let config = confirmation_dialog_config("Continue with update?", body.confirm_label());
    let mut state = DialogState::new(body);
    state.register_button(0);
    state.register_button(1);
    state.register_child(0);
    state.show();
    render_dialog_backdrop(frame);
    {
        let mut dialog = PopupDialog::new(&config, &mut state, draw_update_confirmation_body);
        dialog.render(frame);
    }
    render_update_confirmation_footer(frame);
}

pub(super) fn render_removal_confirmation_preview(frame: &mut ratatui::Frame<'_>) {
    let config = confirmation_dialog_config("Remove map component?", "Remove files");
    let mut state = DialogState::new(removal_confirmation_body(
        indoc! {"
            Example Watch (006-TEST-01)
            desktop-mounted MTP at mounted-mtp:synthetic-watch"},
        "00000000000000000000000000000001",
        "Example Regional Map",
        "2 files",
        "35.20 MB",
        ".tmp/removal-run",
    ));
    state.register_button(0);
    state.register_button(1);
    state.show();
    render_dialog_backdrop(frame);
    {
        let mut dialog = PopupDialog::new(&config, &mut state, draw_confirmation_body);
        dialog.render(frame);
    }
    render_confirmation_footer(frame);
}

pub(super) fn render_pipeline_probe_confirmation_preview(frame: &mut ratatui::Frame<'_>) {
    let config = confirmation_dialog_config("Write to device?", "Write to device");
    let mut state = DialogState::new(ConfirmationBody::message(PIPELINE_PROBE_WRITE_WARNING));
    state.register_button(0);
    state.register_button(1);
    state.show();
    render_dialog_backdrop(frame);
    {
        let mut dialog = PopupDialog::new(&config, &mut state, draw_confirmation_body);
        dialog.render(frame);
    }
    render_confirmation_footer(frame);
}
pub(super) fn render_abort_preview(frame: &mut ratatui::Frame<'_>) {
    let config = abort_dialog_config();
    let mut state = DialogState::new(AbortBody {
        operation: OperationView {
            stage: Some(OperationStage::Download),
            state: Some(ProgressState::Advanced),
            kind: ProgressEventKind::default(),
            label: "Downloading Mock Cycle Map Europe".to_owned(),
            path: Some("Garmin/Mock/europe.img".to_owned()),
            recorded_at: None,
            duration: None,
        },
    });
    state.register_button(0);
    state.register_button(1);
    state.show();
    render_dialog_backdrop(frame);
    {
        let mut dialog = PopupDialog::new(&config, &mut state, draw_abort_body);
        dialog.render(frame);
    }
    render_abort_footer(frame);
}
