//! Deployment backups live in the owner menu without occupying primary navigation.

use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use garmin_color::swatch;
use garmin_i18n::{Language, Translations};
use garmin_service_api::{DeviceSnapshot, InspectionState};
use garmin_ui::{profile, shell, workspace};

#[test]
fn backup_is_an_owner_only_secondary_action() {
    for owner in [false, true] {
        let intl = Translations::bundled()
            .expect("catalog")
            .formatter(Language::English)
            .expect("English");
        let profiles = [profile::ProfileProps {
            display_name: "Alex Rider",
            accent: swatch::cyan::G40,
            avatar: None,
        }];
        let devices = [DeviceSnapshot {
            key: "mock-cycle".to_owned(),
            name: "Mock Cycle".to_owned(),
            identifier: None,
            software_version: None,
            inspection: InspectionState::Ready,
            inspection_error: None,
            report: None,
            capabilities: Vec::new(),
            storages: Vec::new(),
        }];
        let mut installed = false;
        let mut view = Harness::builder().with_size([960.0, 720.0]).build_ui_state(
            move |ui, action: &mut Option<shell::Action>| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                let output = workspace::show(
                    ui,
                    &workspace::Props {
                        product_name: "Garmin Toolkit Demo",
                        intl: &intl,
                        profiles: &profiles,
                        selected_profile: 0,
                        profile_menu_expanded: true,
                        page: &workspace::Page::Activities,
                        navigation: shell::Navigation::Expanded,
                        devices: &devices,
                        backup_enabled: owner,
                        window_controls: None,
                    },
                    |_| {},
                );
                if let Some(chosen) = output.action {
                    *action = Some(chosen);
                }
            },
            None,
        );
        view.run();
        let device = view.get(By::new().predicate(|node| node.author_id() == Some("navigation.2")));
        assert_eq!(
            device.accesskit_node().label().as_deref(),
            Some("Mock Cycle")
        );
        let backup =
            view.query(By::new().predicate(|node| node.author_id() == Some("profile.backup")));
        if owner {
            backup.expect("owner backup action").click();
            view.run_steps(3);
            assert_eq!(
                *view.state(),
                Some(shell::Action::Profile(profile::Action::Backup))
            );
        } else {
            assert!(backup.is_none());
        }
    }
}
