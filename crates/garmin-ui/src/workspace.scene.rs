use gallery::prelude::*;
use garmin_color::swatch;
use garmin_i18n::format_message;
use garmin_service_api::{DeviceSnapshot, InspectionState};
use garmin_ui::{profile, shell, workspace};

scene_meta! { title: "Application / Workspace" }

#[scene(default)]
fn desktop(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show(ctx, ui, globals, "Garmin Toolkit", true);
}

#[scene]
fn web(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show(ctx, ui, globals, "Garmin Toolkit", false);
}

#[scene]
fn web_demo(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show(ctx, ui, globals, "Garmin Toolkit Demo", false);
}

fn show(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    product_name: &str,
    native_controls: bool,
) {
    let intl = globals.intl();
    let activities = format_message!(&intl, default_message: "Activities");
    let content = format_message!(&intl, default_message: "Shared application content");
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
    let controls = shell::WindowControls {
        maximized: false,
        minimize_label: "Minimize window",
        maximize_label: "Maximize window",
        restore_label: "Restore window",
        close_label: "Close window",
    };
    stage!(ctx, ui, (960, 600), |ui| {
        let _ = workspace::show(
            ui,
            &workspace::Props {
                product_name,
                intl: &intl,
                profiles: &profiles,
                selected_profile: 0,
                profile_menu_expanded: false,
                page: &workspace::Page::Activities,
                navigation: shell::Navigation::Expanded,
                devices: &devices,
                window_controls: native_controls.then_some(&controls),
            },
            |ui| {
                ui.heading(&activities);
                ui.add_space(16.0);
                ui.label(&content);
            },
        );
    });
}
