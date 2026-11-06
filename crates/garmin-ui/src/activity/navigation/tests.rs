use super::*;

#[test]
fn archive_stays_inside_its_workspace_and_escape_closes_it() {
    let translations = garmin_i18n::Translations::bundled().unwrap();
    let intl = translations
        .formatter(garmin_i18n::Language::Czech)
        .unwrap();
    for dark in [true, false] {
        let context = egui::Context::default();
        crate::install(&context);
        let bounds = egui::Rect::from_min_size(egui::pos2(48.0, 64.0), egui::vec2(320.0, 600.0));
        let mut navigation = Navigation::default();
        navigation.open_archive();
        let mut id = egui::Id::NULL;
        for frame in 0..5 {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 960.0),
                        )),
                        events: if frame == 4 {
                            vec![egui::Event::Key {
                                key: egui::Key::Escape,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            }]
                        } else {
                            Vec::new()
                        },
                        ..Default::default()
                    },
                    |ui| {
                        ui.scope_builder(egui::UiBuilder::new().max_rect(bounds), |ui| {
                            ui.set_clip_rect(bounds);
                            *ui.visuals_mut() = if dark {
                                egui::Visuals::dark()
                            } else {
                                egui::Visuals::light()
                            };
                            id = ui.id().with("activity-archive");
                            let _ = navigation.archive(
                                ui,
                                &intl,
                                &WorkspaceProps {
                                    today: jiff::civil::date(2026, 11, 6),
                                    items: &[],
                                    presentations: &[],
                                    selected: None,
                                    recording: None,
                                    recording_key: None,
                                    units: garmin_model::identity::UnitSystem::Metric,
                                    empty_list: "",
                                    empty_detail: "",
                                    no_route: "",
                                },
                            );
                        });
                    },
                )
                .drop_without_applying_deltas();
            if frame == 3 {
                let dialog = context.memory(|memory| memory.area_rect(id)).unwrap();
                assert!(
                    bounds.contains_rect(dialog),
                    "{dialog:?} overflows {bounds:?}"
                );
                assert!(navigation.archive_open);
            }
        }
        assert!(!navigation.archive_open, "Escape must dismiss the archive");
    }
}
