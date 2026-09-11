use std::sync::{Mutex, OnceLock};

use gallery::prelude::*;
use garmin_ui::notification;

scene_meta! { title: "Components / Feedback / Notifications" }

#[scene]
fn states(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(560.0);
        for props in [
            notification::Props {
                kind: notification::Kind::Information,
                title: "Device scan is ready",
                detail: Some("Choose a profile before importing files."),
            },
            notification::Props {
                kind: notification::Kind::Success,
                title: "Imported 3 activities",
                detail: None,
            },
            notification::Props {
                kind: notification::Kind::Warning,
                title: "Imported 2 of 3 files",
                detail: Some("One malformed FIT file was preserved but rejected."),
            },
            notification::Props {
                kind: notification::Kind::Error,
                title: "Import failed",
                detail: Some("The selected directory could not be read."),
            },
        ] {
            notification::show(ui, &props);
            ui.add_space(12.0);
        }
    });
}

#[scene]
fn actionable(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let kind = kind(ctx.buttons("kind", &["information", "success", "warning", "error"], 0));
    let detail = ctx.toggle("detail", true);
    let action = ctx.toggle("action", true);
    let closable = ctx.toggle("closable", true);

    stage!(ctx, ui, |ui| {
        ui.set_width(320.0);
        let _ = notification::actionable(
            ui,
            &notification::ActionableProps {
                kind,
                title: "Garmin Edge 1050 connected",
                detail: detail.then_some("Inspecting…"),
                action: action.then_some("View device"),
                closable,
            },
        );
    });
}

#[scene(default)]
fn toast_stack(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let add = ctx.buttons("add", &["none", "device", "saved", "warning", "failure"], 0);
    let mode = ctx.buttons("mode", &["automatic", "collapsed", "expanded"], 0);

    let _ = ctx.button("reset", || {
        *toast_scene()
            .lock()
            .expect("toast gallery state is poisoned") = seeded_toast_scene();
    });
    if let Some(toast) = toast(add) {
        toast_scene()
            .lock()
            .expect("toast gallery state is poisoned")
            .toasts
            .push(toast);
        let _ = ctx.set_select_index("add", 0);
    }

    stage!(ctx, ui, (760, 480), |ui| {
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);

        let mut state = toast_scene()
            .lock()
            .expect("toast gallery state is poisoned");
        let _ = state.toasts.show_in(
            ui.ctx(),
            egui::Id::new("notification-gallery-stack"),
            rect,
            match mode {
                1 => notification::StackMode::Collapsed,
                2 => notification::StackMode::Expanded,
                _ => notification::StackMode::Automatic,
            },
        );
    });
}

#[derive(Default)]
struct ToastScene {
    toasts: notification::Toasts,
}

fn toast_scene() -> &'static Mutex<ToastScene> {
    static STATE: OnceLock<Mutex<ToastScene>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(seeded_toast_scene()))
}

fn seeded_toast_scene() -> ToastScene {
    let mut state = ToastScene::default();
    state.toasts.push(
        notification::Toast::new(
            notification::Kind::Information,
            "Garmin Edge 1050 connected",
        )
        .detail("Inspecting…")
        .action("View device"),
    );
    state.toasts.push(
        notification::Toast::new(notification::Kind::Success, "Profile settings saved")
            .persistent(),
    );
    state.toasts.push(
        notification::Toast::new(notification::Kind::Warning, "One FIT file needs attention")
            .detail("The source file was preserved but could not be imported.")
            .persistent(),
    );
    state
}

fn toast(index: usize) -> Option<notification::Toast> {
    match index {
        1 => Some(
            notification::Toast::new(
                notification::Kind::Information,
                "Garmin Edge 1050 connected",
            )
            .detail("Inspecting…")
            .action("View device"),
        ),
        2 => Some(notification::Toast::new(
            notification::Kind::Success,
            "Profile settings saved",
        )),
        3 => Some(
            notification::Toast::new(notification::Kind::Warning, "One FIT file needs attention")
                .detail("The source file was preserved but could not be imported."),
        ),
        4 => Some(
            notification::Toast::new(notification::Kind::Error, "Device scan failed")
                .detail("The Garmin storage disappeared during the scan."),
        ),
        _ => None,
    }
}

const fn kind(index: usize) -> notification::Kind {
    match index {
        1 => notification::Kind::Success,
        2 => notification::Kind::Warning,
        3 => notification::Kind::Error,
        _ => notification::Kind::Information,
    }
}
