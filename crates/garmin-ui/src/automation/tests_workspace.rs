//! Exercise responsive selection against the production workspace and drawers.
use super::*;
use crate::activity;
use crate::activity::map_runtime;
use garmin_i18n::{Language, Translations};
use garmin_model::{
    activity::{
        ActivityDuration, ActivityMetrics, ActivitySport, ActivitySummary, ActivityTotals,
        TimeRange,
    },
    identity::UnitSystem,
    value::Timestamp,
};

struct NoTiles;
impl map_runtime::Backend for NoTiles {
    fn submit(&self, task: map_runtime::TileTask) {
        task.complete_encoded(Ok(Vec::new()));
    }
}

#[test]
fn responsive_selection_opens_real_drawers_and_preserves_the_selected_activity() {
    let context = Context::default();
    crate::install(&context);
    context.add_plugin(Driver::default());
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let presentations = [presentation(&intl)];
    let items = presentations
        .iter()
        .map(activity::Presentation::item_props)
        .collect::<Vec<_>>();
    let runtime = map_runtime::MapRuntimeHandle::new(NoTiles, map_runtime::Renderer::software());
    let mut workspace = activity::Workspace::new(&runtime);
    let mut selected = 0;
    context
        .plugin::<Driver>()
        .lock()
        .start_steps("workspace-resize", selection_steps())
        .unwrap();
    let mut size = egui::vec2(1100.0, 720.0);
    let mut hidden_row_seen = false;
    let mut open_drawer_seen = false;
    for tick in 0..600 {
        let mut output = context.run_ui(
            RawInput {
                time: Some(f64::from(tick) / 60.0),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                focused: true,
                ..Default::default()
            },
            |ui| {
                let rect = Rect::from_min_max(
                    egui::pos2(256.0, 200.0),
                    (size - egui::vec2(24.0, 24.0)).to_pos2(),
                );
                let mut body = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                body.set_clip_rect(rect);
                if let Some(activity::Action::Select(index)) = workspace.show(
                    &mut body,
                    &intl,
                    &activity::WorkspaceProps {
                        items: &items,
                        presentations: &presentations,
                        selected: Some(selected),
                        recording: None,
                        recording_key: None,
                        units: UnitSystem::Metric,
                        empty_list: "No activities",
                        empty_detail: "Select an activity",
                        no_route: "No route",
                    },
                ) {
                    selected = index;
                }
            },
        );
        for command in &output.viewport_output[&egui::ViewportId::ROOT].commands {
            if let egui::ViewportCommand::InnerSize(request) = command {
                size = *request;
            }
        }
        output.textures_delta.clear();
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        if let Some((_, state)) = lookup(
            driver.tree.as_ref(),
            "activity.list.toggle",
            Rect::EVERYTHING,
        )
        .unwrap()
        {
            let row = lookup(driver.tree.as_ref(), "activity.0", Rect::EVERYTHING).unwrap();
            hidden_row_seen |= state.as_deref() == Some("closed") && row.is_none();
            open_drawer_seen |= state.as_deref() == Some("open") && row.is_some();
        }
        if !driver.running() {
            break;
        }
    }
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().unwrap();
    assert_eq!(report.state, "passed", "{:?}", report.failure);
    assert_eq!(report.completed, report.total);
    assert!(hidden_row_seen);
    assert!(open_drawer_seen);
    assert_eq!(selected, 0);
    assert!(
        lookup(driver.tree.as_ref(), "activity.0", Rect::EVERYTHING)
            .unwrap()
            .is_some()
    );
}

fn presentation(intl: &garmin_i18n::Intl) -> activity::Presentation {
    let duration = ActivityDuration::from_milliseconds(60_000);
    let summary = ActivitySummary::from_parts(
        ActivitySport::Cycling,
        TimeRange::from_parts(
            Timestamp::from_unix_milliseconds(0).unwrap(),
            Timestamp::from_unix_milliseconds(60_000).unwrap(),
        )
        .unwrap(),
        ActivityTotals::from_parts(duration, duration, None, None, None, None).unwrap(),
        ActivityMetrics::default(),
    );
    activity::Presentation::from_summary(summary, "Test", intl, UnitSystem::Metric)
}

fn selection_steps() -> Vec<Step> {
    let mut steps = Vec::new();
    for (phase, width, height) in [("narrow", 720.0, 640.0), ("wide", 1100.0, 720.0)] {
        steps.push(Step {
            phase,
            target: "viewport".into(),
            action: Action::Resize { width, height },
            after: 0.1,
        });
        steps.extend(scenarios::responsive_selection_steps(phase));
    }
    steps
}
