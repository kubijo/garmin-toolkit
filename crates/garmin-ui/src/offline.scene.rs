use gallery::prelude::*;
use garmin_color::swatch;
use garmin_i18n::format_message;
use garmin_service_api::{DeviceSnapshot, InspectionState};
use garmin_ui::{offline, profile, shell, workspace};

scene_meta! { title: "Application / States / Offline" }

#[scene(default)]
fn disconnected(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let intl = globals.intl();
    let title = format_message!(&intl, default_message: "Connection lost");
    let message = format_message!(&intl, default_message: "Reconnecting automatically…");
    let activities = format_message!(&intl, default_message: "Activities");
    let content = format_message!(
        &intl,
        default_message: "Your recent activities will reappear when the connection returns."
    );
    let elapsed = format_message!(
        &intl,
        default_message: "Offline for {duration}",
        values: { duration: "01:42" },
    );
    let profiles = [profile::ProfileProps {
        display_name: "Alex Rider",
        accent: swatch::cyan::G40,
        avatar: None,
    }];
    let devices = [DeviceSnapshot {
        key: "edge-1050".to_owned(),
        name: "Garmin Edge 1050".to_owned(),
        identifier: Some(36_264_719),
        software_version: Some(3_220),
        inspection: InspectionState::Ready,
        capabilities: Vec::new(),
        storages: Vec::new(),
    }];
    stage!(ctx, ui, (960, 600), |ui| {
        ui.disable();
        let _ = workspace::show(
            ui,
            &workspace::Props {
                product_name: "Garmin Toolkit",
                intl: &intl,
                profiles: &profiles,
                selected_profile: 0,
                profile_menu_expanded: false,
                page: &workspace::Page::Activities,
                navigation: shell::Navigation::Expanded,
                devices: &devices,
                window_controls: None,
            },
            |ui| {
                ui.heading(&activities);
                ui.add_space(16.0);
                ui.label(&content);
            },
        );
        offline::show(
            ui,
            egui::Id::new("gallery-offline"),
            &offline::Props {
                title: &title,
                message: &message,
                elapsed: &elapsed,
            },
        );
    });
}
