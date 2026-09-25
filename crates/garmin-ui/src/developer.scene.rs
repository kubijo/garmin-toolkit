use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_ui::developer;

scene_meta! { title: "Application / Developer tools" }

#[scene]
fn header_button(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, Stage::Fixed(egui::vec2(240.0, 100.0)), |ui| {
        ui.horizontal(|ui| {
            for (label, highlight) in [("Rest", false), ("Hover", true)] {
                ui.push_id(label, |ui| {
                    ui.vertical(|ui| {
                        ui.label(label);
                        egui::Frame::new()
                            .fill(ui.visuals().panel_fill)
                            .show(ui, |ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(32.0, 32.0),
                                    egui::Sense::hover(),
                                );
                                let response = developer::header_button(ui, rect);
                                if highlight {
                                    ui.ctx().highlight_widget(response.id);
                                }
                            });
                    });
                });
            }
        });
    });
}

thread_local! {
    static PANELS: crate::SceneState<developer::State, 3> = const { crate::SceneState::empty() };
}

#[scene]
fn browser(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    preview(ctx, ui, 0, 620.0, &globals.intl());
}

#[scene]
fn desktop(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    preview(ctx, ui, 1, 620.0, &globals.intl());
}

#[scene]
fn disconnected(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    preview(ctx, ui, 2, 380.0, &globals.intl());
}

fn preview(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    slot: usize,
    default_width: f32,
    intl: &garmin_i18n::Intl,
) {
    let width = ctx.slider("width", default_width, 320.0, 960.0, 1.0);
    if ui
        .ctx()
        .plugin_opt::<garmin_ui::automation::Driver>()
        .is_none()
    {
        ui.ctx()
            .add_plugin(garmin_ui::automation::Driver::default());
    }
    stage!(ctx, ui, Stage::Fixed(egui::vec2(width, 660.0)), |ui| {
        PANELS.with_scene(
            slot,
            || sample(slot),
            |state| {
                if state.native {
                    garmin_ui::window::surface(ui, |ui| {
                        let labels = garmin_ui::shell::WindowLabels::new(intl);
                        let controls = labels.props(ui.ctx());
                        let _ = garmin_ui::shell::window_header(ui, "Developer tools", &controls);
                        developer::contents(ui, state);
                    });
                } else {
                    developer::contents(ui, state);
                }
            },
        );
    });
}

fn sample(slot: usize) -> developer::State {
    use garmin_model::logging::{Level, Record};
    developer::State {
        native: slot != 0,
        connected: slot != 2,
        server: (slot == 1).then(|| "http://127.0.0.1:43127".into()),
        log_error: (slot == 2).then(|| "Connection lost. Retained records remain available; the stream will reconnect automatically.".into()),
        export_error: (slot == 2).then(|| "Log export interrupted: records expired during export. Retry the export.".into()),
        server_error: (slot == 2).then(|| "Could not bind the control listener.".into()),
        gap: slot == 2,
        debug: "Desktop · Vulkan · scale 1.5".into(),
        viewport: [1280.0, 720.0],
        scale: 1.5,
        logs: [
            (Level::Info, "garmin_desktop", "Application started"),
            (Level::Info, "automation", "activity-smoke started"),
            (Level::Warn, "map-worker", "Tile request failed; retrying after the connection becomes available"),
        ].into_iter().enumerate().map(|(index, (level, component, message))| Record {
            sequence: index as u64 + 1,
            timestamp_ms: 1_790_166_467_209,
            level,
            component: component.into(),
            source: if slot == 0 { "browser/worker" } else { "desktop" }.into(),
            session: "linux-acceptance-session".into(),
            source_sequence: index as u64 + 1,
            message: message.into(),
            fields: std::collections::BTreeMap::from([("details".into(), "A long diagnostic value must wrap within the panel without creating a second horizontal or vertical scroll area.".into())]),
        }).collect(),
        ..Default::default()
    }
}
