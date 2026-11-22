use gallery::prelude::*;
use garmin_color::swatch;
use garmin_i18n::format_message;
use garmin_service_api::{DeviceSnapshot, InspectionState, firmware::AvailableUpdate};
use garmin_ui::{firmware::Notices, notification, profile, shell, workspace};
use std::sync::{Arc, Mutex};

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
    let update = ctx.toggle("firmware update", false);
    let rail = ctx.toggle("collapsed", false);
    let connect = ctx.toggle("Garmin Connect configured", false);
    let activities = format_message!(&intl, default_message: "Activities");
    let content = format_message!(&intl, default_message: "Shared application content");
    let profiles = [profile::ProfileProps {
        display_name: "Alex Rider",
        accent: swatch::cyan::G40,
        avatar: None,
    }];
    let devices = [DeviceSnapshot {
        key: "mock-cycle-o-matic-9000".to_owned(),
        name: "Mock Cycle-o-Matic 9000".to_owned(),
        identifier: Some(36_264_719),
        firmware_update: update.then(|| AvailableUpdate {
            attachment: uuid::Uuid::nil(),
            version: "33.22".into(),
        }),
        software_version: Some(3_220),
        inspection: InspectionState::Ready,
        inspection_error: None,
        report: None,
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
        let bounds = shell::overlay_bounds(ui.available_rect_before_wrap());
        let _ = workspace::show(
            ui,
            &workspace::Props {
                product_name,
                intl: &intl,
                profiles: &profiles,
                selected_profile: 0,
                profile_menu_expanded: false,
                page: &workspace::Page::Activities,
                navigation: if rail {
                    shell::Navigation::Rail
                } else {
                    shell::Navigation::Expanded
                },
                devices: &devices,
                backup_enabled: false,
                garmin_connect_configured: connect,
                window_controls: native_controls.then_some(&controls),
            },
            |ui| {
                ui.heading(&activities);
                ui.add_space(16.0);
                ui.label(&content);
            },
        );
        if update {
            let id = ui.id().with("firmware-notification");
            let state = ui.data_mut(|data| {
                data.get_temp_mut_or_default::<Arc<Mutex<(Notices, notification::Toasts)>>>(id)
                    .clone()
            });
            let (notices, toasts) = &mut *state
                .lock()
                .expect("workspace notification state is poisoned");
            notices.sync(&intl, &devices, toasts, true);
            for event in toasts.show_in(ui.ctx(), id, bounds, notification::StackMode::Automatic) {
                let _ = notices.handle(event);
            }
        }
    });
}
