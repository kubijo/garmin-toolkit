use super::*;
use garmin_i18n::{Language, Translations};
use garmin_model::device::{
    DeviceInspection, DeviceStateSnapshot, DeviceStorageState, IdentityInspection,
    InspectionFailure, InspectionFailureKind, InspectionSection, StorageCapacity,
    StorageInspection,
};

fn snapshot() -> DeviceSnapshot {
    let storage = DeviceStorageState {
        id: "internal".to_owned(),
        label: "Internal storage".to_owned(),
        capacity: StorageCapacity::new(32_000_000_000, 8_600_000_000),
        writable: Some(true),
    };
    DeviceSnapshot {
        key: "fixture".to_owned(),
        name: "Mock Watch-o-Matic 9000".to_owned(),
        identifier: Some(42),
        software_version: Some(1870),
        inspection: InspectionState::Failed,
        inspection_error: None,
        capabilities: Vec::new(),
        storages: vec![storage.clone()],
        report: Some(DeviceInspection {
            manifest: InspectionSection::Missing,
            storage: InspectionSection::Available(DeviceStateSnapshot {
                storages: vec![storage],
            }),
            toolkit: vec![StorageInspection {
                storage_id: "internal".to_owned(),
                namespace: InspectionSection::Missing,
                identity: InspectionSection::Available(IdentityInspection {
                    device_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
                    device_digest: "a".repeat(32),
                    paired_user_id: Some("00000000-0000-4000-8000-000000000002".parse().unwrap()),
                    verified: true,
                }),
                marker: InspectionSection::Missing,
                transaction: InspectionSection::Unavailable(InspectionFailure {
                    kind: InspectionFailureKind::UnsupportedVersion,
                    message: "unsupported active-transaction device-state version 99".to_owned(),
                }),
            }],
        }),
    }
}

fn frame(
    context: &egui::Context,
    intl: &Intl,
    snapshot: &DeviceSnapshot,
    width: f32,
    pairing_available: bool,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Action>) {
    let mut action = None;
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 1000.0),
            )),
            events,
            ..egui::RawInput::default()
        },
        |ui| {
            action = show_snapshot(ui, intl, snapshot, false, pairing_available);
            assert!(
                ui.min_rect().right() <= width,
                "device view overflowed its available width"
            );
        },
    );
    (output, action)
}

#[test]
fn partial_inspection_survives_postcard_transport() {
    let snapshot = snapshot();
    let bytes = postcard::to_stdvec(&snapshot).unwrap();
    assert_eq!(
        postcard::from_bytes::<DeviceSnapshot>(&bytes).unwrap(),
        snapshot
    );
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "AccessKit exposes egui's f32 bounds as f64"
)]
fn refresh_is_clickable_in_partial_views_and_disabled_while_running() {
    let translations = Translations::bundled().unwrap();
    for language in [Language::English, Language::Czech] {
        let intl = translations.formatter(language).unwrap();
        for theme in [egui::ThemePreference::Light, egui::ThemePreference::Dark] {
            for width in [360.0, 720.0] {
                let context = egui::Context::default();
                crate::install(&context);
                context.enable_accesskit();
                context.set_theme(theme);
                let mut snapshot = snapshot();
                let (output, _) = frame(&context, &intl, &snapshot, width, false, Vec::new());
                let bounds = output
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .find(|(_, node)| node.author_id() == Some("device.refresh"))
                    .unwrap()
                    .1
                    .bounds()
                    .unwrap();
                let mut position = egui::pos2(
                    bounds.x0.midpoint(bounds.x1) as f32,
                    bounds.y0.midpoint(bounds.y1) as f32,
                );
                output.drop_without_applying_deltas();
                let (hover, _) = frame(
                    &context,
                    &intl,
                    &snapshot,
                    width,
                    false,
                    vec![egui::Event::PointerMoved(position)],
                );
                assert_eq!(
                    hover.platform_output.cursor_icon,
                    egui::CursorIcon::PointingHand
                );
                hover.drop_without_applying_deltas();
                for running in [false, true] {
                    if running {
                        snapshot.inspection = InspectionState::Running;
                        let (output, _) =
                            frame(&context, &intl, &snapshot, width, false, Vec::new());
                        let node = &output
                            .platform_output
                            .accesskit_update
                            .as_ref()
                            .unwrap()
                            .nodes
                            .iter()
                            .find(|(_, node)| node.author_id() == Some("device.refresh"))
                            .unwrap()
                            .1;
                        assert!(node.is_disabled());
                        let bounds = node.bounds().unwrap();
                        position = egui::pos2(
                            bounds.x0.midpoint(bounds.x1) as f32,
                            bounds.y0.midpoint(bounds.y1) as f32,
                        );
                        output.drop_without_applying_deltas();
                    }
                    for pressed in [true, false] {
                        let (output, action) = frame(
                            &context,
                            &intl,
                            &snapshot,
                            width,
                            false,
                            vec![egui::Event::PointerButton {
                                pos: position,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            }],
                        );
                        assert_eq!(action, (!running && !pressed).then_some(Action::Refresh));
                        output.drop_without_applying_deltas();
                    }
                }
            }
        }
    }
}

#[test]
fn refresh_supports_keyboard_activation() {
    let context = egui::Context::default();
    crate::install(&context);
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let mut snapshot = snapshot();
    snapshot.report = None;
    let (output, _) = frame(&context, &intl, &snapshot, 720.0, false, Vec::new());
    output.drop_without_applying_deltas();
    for key in [egui::Key::Tab, egui::Key::Enter] {
        let (output, action) = frame(
            &context,
            &intl,
            &snapshot,
            720.0,
            false,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        if key == egui::Key::Enter {
            assert_eq!(action, Some(Action::Refresh));
        }
        output.drop_without_applying_deltas();
    }
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "AccessKit exposes egui's f32 bounds as f64"
)]
fn pairing_action_is_a_clickable_button_at_narrow_and_wide_widths() {
    let translations = Translations::bundled().unwrap();
    for language in [Language::English, Language::Czech] {
        let intl = translations.formatter(language).unwrap();
        for width in [320.0, 720.0] {
            let context = egui::Context::default();
            crate::install(&context);
            context.enable_accesskit();
            let mut snapshot = snapshot();
            snapshot.inspection = InspectionState::Ready;
            snapshot.report = None;
            let (output, _) = frame(&context, &intl, &snapshot, width, true, Vec::new());
            let node = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(_, node)| node.author_id() == Some("device.pair"))
                .unwrap()
                .1;
            assert!(!node.is_disabled());
            let bounds = node.bounds().unwrap();
            let position = egui::pos2(
                bounds.x0.midpoint(bounds.x1) as f32,
                bounds.y0.midpoint(bounds.y1) as f32,
            );
            output.drop_without_applying_deltas();
            let (hover, _) = frame(
                &context,
                &intl,
                &snapshot,
                width,
                true,
                vec![egui::Event::PointerMoved(position)],
            );
            assert_eq!(
                hover.platform_output.cursor_icon,
                egui::CursorIcon::PointingHand
            );
            hover.drop_without_applying_deltas();
            for pressed in [true, false] {
                let (output, action) = frame(
                    &context,
                    &intl,
                    &snapshot,
                    width,
                    true,
                    vec![egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                assert_eq!(action, (!pressed).then_some(Action::Pair));
                output.drop_without_applying_deltas();
            }
        }
    }
}
