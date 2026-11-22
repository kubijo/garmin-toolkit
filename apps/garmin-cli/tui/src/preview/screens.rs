use garmin_device::{DeviceSummary, TransportKind};
use garmin_i18n::format_message;
use garmin_model::artifact::ByteCount;
use garmin_model::map::MapOperation;
use garmin_progress::{OperationStage, ProgressEventKind, ProgressState};
use indoc::indoc;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{ListState, TableState};
use ratatui_interact::components::{DialogState, PopupDialog};

use crate::{
    AbortBody, ConfirmationBody, MapActionMode, MapCacheAvailability, MapChoice, MapChoiceAction,
    MapVersionTone, OperationView, PIPELINE_PROBE_WRITE_WARNING, PendingRecoveryActions,
    RecoveryOperation, SelectedMapAction, abort_dialog_config, confirmation_dialog_config,
    draw_abort_body, draw_confirmation_body, draw_map_selection, draw_update_confirmation_body,
    garmin_contact_confirmation_body, pending_recovery_confirmation_body,
    pending_recovery_dialog_config, removal_confirmation_body, render_abort_footer,
    render_confirmation_footer, render_device_selection, render_dialog_backdrop,
    render_pending_recovery_footer, render_update_confirmation_footer, selected_formatter,
    update_confirmation_body, update_device_rows,
};

const PREVIEW_MAP_SERVICE_URL: &str = "https://omt.garmin.com/api/maps/universal/update";

/// Decode a firmware gallery fixture using the host catalog model.
/// # Panics
/// The screen must select a firmware fixture with valid inventory and deliverables.
#[must_use]
pub fn firmware_components(screen: super::PreviewScreen) -> Vec<garmin_model::firmware::Component> {
    use super::PreviewScreen;
    use garmin_device::parse_manifest;
    use garmin_fixtures::firmware::{EDGE_32_20, FENIX_24_20, FENIX_GSP_22_44};
    use garmin_update::firmware::FirmwareCatalog;
    let capture = match screen {
        PreviewScreen::FirmwareEdge => EDGE_32_20,
        PreviewScreen::FirmwareWatchComponents => FENIX_24_20,
        PreviewScreen::FirmwareWatchSystem => FENIX_GSP_22_44,
        _ => unreachable!("firmware catalog preview"),
    };
    let device = parse_manifest(
        capture.inventory,
        TransportKind::MassStorage,
        "fixture".into(),
    )
    .expect("fixture inventory");
    let catalog = FirmwareCatalog::from_json(capture.catalog.as_bytes()).expect("fixture catalog");
    catalog
        .components(device.capabilities())
        .expect("fixture components")
        .iter()
        .map(|item| item.snapshot(None).expect("fixture size"))
        .collect()
}

pub(super) fn render_firmware_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    screen: super::PreviewScreen,
) {
    let mut choices = Vec::new();
    let mut actions = Vec::new();
    let intl = selected_formatter();
    for item in firmware_components(screen) {
        choices.push(MapChoice::firmware(&item, &intl));
        actions.push(if item.system_bundle {
            MapChoiceAction::Change
        } else {
            MapChoiceAction::Keep
        });
    }
    let selected = actions
        .iter()
        .position(|a| *a == MapChoiceAction::Change)
        .unwrap_or(0);
    let mut state = TableState::default().with_selected(Some(selected));
    let areas = Layout::vertical([Constraint::Min(5), Constraint::Length(4)]).split(area);
    draw_map_selection(
        frame,
        (areas[0], areas[1]),
        &choices,
        &actions,
        &mut state,
        false,
        MapActionMode::Firmware,
    );
}

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
    let mut state = DialogState::new(pending_recovery_confirmation_body(
        indoc! {"
            Mock Watch-o-Matic 9000 (006-FAKE-01)
            desktop-mounted MTP at mounted-mtp:mock-watch"},
        RecoveryOperation::Update,
        "01953ce506d6d6da6c7bfafaf8eafd4e",
        Some(".tmp/mock-watch-recovery"),
        actions,
    ));
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
    let intl = selected_formatter();
    let rows = if empty {
        Vec::from([format_message!(
            &intl,
            default_message: "No Garmin device is currently visible"
        )])
    } else {
        update_device_rows(
            &intl,
            &[
                DeviceSummary {
                    transport: TransportKind::MountedMtp,
                    model: "Mock Watch-o-Matic 9000".to_owned(),
                    part_number: Some("006-FAKE-01".to_owned()),
                    software_version: Some("99.01".to_owned()),
                    location: "mounted-mtp:synthetic-watch".to_owned(),
                },
                DeviceSummary {
                    transport: TransportKind::MountedMtp,
                    model: "Mock Cycle-o-Matic 9000".to_owned(),
                    part_number: Some("006-FAKE-02".to_owned()),
                    software_version: Some("99.02".to_owned()),
                    location: "mounted-mtp:synthetic-cycle".to_owned(),
                },
            ],
        )
    };
    let title = format_message!(&intl, default_message: "Select an update device");
    let mut state = ListState::default().with_selected((!empty).then_some(0));
    render_device_selection(frame, area, &rows, &title, true, &mut state);
}

pub(super) fn render_map_preview(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let choices = [
        MapChoice {
            name: "Base maps".to_owned(),
            version_transition: "(9.00) → 9.00".to_owned(),
            version_tone: MapVersionTone::UpToDate,
            description: "Base maps · 2 files · 53.95 MB".to_owned(),
            operation: MapOperation::Reinstall,
            unavailable_reason: None,
            can_remove: false,
            cache: None,
        },
        MapChoice {
            name: "TopoActive Central Europe".to_owned(),
            version_transition: "(2024.10) → 2026.11".to_owned(),
            version_tone: MapVersionTone::Outdated,
            description: "Europe · TopoActive · 5 files · 5.30 GB".to_owned(),
            operation: MapOperation::Update,
            unavailable_reason: None,
            can_remove: true,
            cache: Some(MapCacheAvailability {
                cached_files: 5,
                total_files: 5,
            }),
        },
        MapChoice {
            name: "Garmin Ski Map".to_owned(),
            version_transition: "(unknown) → 2026.10".to_owned(),
            version_tone: MapVersionTone::Unknown,
            description: "Points of Interest · 2 files · 42.76 MB".to_owned(),
            operation: MapOperation::Install,
            unavailable_reason: None,
            can_remove: false,
            cache: Some(MapCacheAvailability {
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
                    StorageCapacity::new(
                        ByteCount::from_gigabytes(32),
                        ByteCount::from_megabytes(8_600),
                    )
                },
            },
            DeviceStorageState {
                id: "card".to_owned(),
                label: "Memory card".to_owned(),
                writable: Some(false),
                capacity: StorageCapacity::new(
                    ByteCount::from_gigabytes(64),
                    ByteCount::from_gigabytes(52),
                ),
            },
        ],
    }
}

pub(super) fn render_confirmation_preview(frame: &mut ratatui::Frame<'_>, backup_enabled: bool) {
    let body = update_confirmation_body(
        indoc! {"
            Mock Watch-o-Matic 9000 (006-FAKE-01)
            desktop-mounted MTP at mounted-mtp:mock-watch"},
        &[
            SelectedMapAction::new("Base maps", MapOperation::Reinstall),
            SelectedMapAction::new("Garmin Ski Map", MapOperation::Install),
            SelectedMapAction::new("TopoActive Central Europe", MapOperation::Update),
        ],
        "952bf20745678c8c8a39bb27016d99ca",
        9,
        "6.40 GB",
        9,
        ".tmp/mock-watch-update",
    )
    .with_backup_plan_ids(
        "952bf20745678c8c8a39bb27016d99ca",
        "59f49870b6577cc75e7d3f21b18e9bcb",
    )
    .with_backup_enabled(backup_enabled);
    let intl = selected_formatter();
    let title = format_message!(&intl, default_message: "Continue with update?");
    let confirm_label = body.confirm_label();
    let config = confirmation_dialog_config(&title, &confirm_label);
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
            Mock Watch-o-Matic 9000 (006-FAKE-01)
            desktop-mounted MTP at mounted-mtp:synthetic-watch"},
        "00000000000000000000000000000001",
        "Example Regional Map",
        2,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::PreviewScreen;
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    #[test]
    fn firmware_write_emphasizes_identifiers_versions_and_total_size() {
        let mut item = firmware_components(PreviewScreen::FirmwareEdge)
            .into_iter()
            .find(|item| item.part_number == "006-D1074-00")
            .unwrap();
        item.installed = Some("40.00".into());
        let mut body =
            ConfirmationBody::firmware_write([&item], item.download_bytes.unwrap(), "Edge 1050");
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal
            .draw(|frame| draw_confirmation_body(frame, frame.area(), &mut body))
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (row, runs) in [
            vec![
                ("006-D1074-00", true),
                ("  Time Zone Map  ", false),
                ("40.00", true),
                (" → ", false),
                ("43.00", true),
            ],
            vec![
                ("569.0 KiB", true),
                (" to stage on ", false),
                ("Edge 1050", true),
            ],
        ]
        .into_iter()
        .enumerate()
        {
            let mut x = 2;
            for (text, bold) in runs {
                for ch in text.chars() {
                    let cell = &buffer[(x, 1 + u16::try_from(row).unwrap())];
                    assert_eq!(cell.symbol(), ch.to_string());
                    assert_eq!(cell.modifier.contains(Modifier::BOLD), bold, "{text}");
                    x += 1;
                }
            }
        }
    }
}
