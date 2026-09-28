//! Exercise the production calendar, pager, and archive through pointer and keyboard input.
use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use gallery::eframe::egui;
use garmin_i18n::{Language, Translations};
use garmin_model::{
    activity::{
        ActivityDuration, ActivityMetrics, ActivitySport, ActivitySummary, ActivityTotals,
        TimeRange,
    },
    identity::UnitSystem,
    value::Timestamp,
};
use garmin_ui::activity;

struct NoTiles;
impl activity::map_runtime::Backend for NoTiles {
    fn fetch(
        &self,
        _: activity::map_runtime::TileCoordinates,
        reply: activity::map_runtime::TileReply,
    ) {
        reply(Ok(activity::map_runtime::TileData::Encoded(Vec::new())));
    }
}

fn harness() -> Harness<'static, usize> {
    let intl = Translations::bundled()
        .expect("bundled translations")
        .formatter(Language::English)
        .expect("English formatter");
    let presentations = [0, 60_000, 259_200_000].map(|milliseconds| {
        // Mid-month keeps all three fixtures in one month in every local zone.
        let time = Timestamp::from_unix_milliseconds(1_252_800_000 + milliseconds)
            .expect("fixture timestamp");
        let duration = ActivityDuration::from_milliseconds(0);
        let summary = ActivitySummary::from_parts(
            ActivitySport::Cycling,
            TimeRange::from_parts(time, time).expect("fixture time range"),
            ActivityTotals::from_parts(duration, duration, None, None, None, None)
                .expect("fixture totals"),
            ActivityMetrics::default(),
        );
        activity::Presentation::from_summary(summary, "Calendar fixture", &intl, UnitSystem::Metric)
    });
    let runtime = activity::map_runtime::MapRuntimeHandle::new(
        NoTiles,
        activity::map_runtime::Renderer::software(),
    );
    let mut workspace = activity::Workspace::new(&runtime);
    let mut installed = false;
    Harness::builder().with_size([900.0, 720.0]).build_ui_state(
        move |ui, selected: &mut usize| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            let items = presentations
                .iter()
                .map(activity::Presentation::item_props)
                .collect::<Vec<_>>();
            if let Some(activity::Action::Select(index)) = workspace.show(
                ui,
                &intl,
                &activity::WorkspaceProps {
                    items: &items,
                    presentations: &presentations,
                    selected: Some(*selected),
                    recording: None,
                    recording_key: None,
                    units: UnitSystem::Metric,
                    empty_list: "Empty",
                    empty_detail: "Select",
                    no_route: "No route",
                },
            ) {
                *selected = index;
            }
        },
        1,
    )
}

#[test]
fn empty_days_ignore_clicks_and_keyboard_can_choose_another_day() {
    let mut harness = harness();
    harness.run();
    let is_day = |id: &str| {
        id.strip_prefix("activity.calendar.")
            .is_some_and(|suffix| suffix.starts_with(|c: char| c.is_ascii_digit()))
    };
    let empty = harness
        .get_all(
            By::new().predicate(|node| node.author_id().is_some_and(is_day) && node.is_disabled()),
        )
        .next()
        .expect("an empty calendar day");
    empty.click();
    harness.run();
    assert_eq!(*harness.state(), 1);
    let mut days = harness
        .get_all(
            By::new().predicate(|node| node.author_id().is_some_and(is_day) && !node.is_disabled()),
        )
        .map(|node| {
            node.accesskit_node()
                .author_id()
                .expect("calendar target")
                .to_owned()
        })
        .collect::<Vec<_>>();
    days.sort();
    assert_eq!(days.len(), 2);
    harness
        .get(By::new().predicate(|node| node.author_id() == Some(days[0].as_str())))
        .click();
    harness.run();
    assert_eq!(
        *harness.state(),
        1,
        "choosing the current day preserves its selected activity"
    );
    harness
        .get(By::new().predicate(|node| node.author_id() == Some(days[1].as_str())))
        .focus();
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(*harness.state(), 2);
}

#[test]
fn pager_and_search_keep_original_activity_indices() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Previous activity").click();
    harness.run();
    assert_eq!(*harness.state(), 0);
    assert!(
        harness
            .get_by_label("Previous activity")
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_label("Next activity").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
    harness.get_by_label("All activities").click();
    harness.run();
    let search = harness
        .get(By::new().predicate(|node| node.author_id() == Some("activity.archive.search")));
    search.focus();
    search.type_text("not an activity");
    harness.run();
    let _ = harness.get_by_label("No matching activities");
    harness.get_by_label("Close").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
    harness.get_by_label("All activities").click();
    harness.run();
    harness
        .get(By::new().predicate(|node| node.author_id() == Some("activity.archive.search")))
        .focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness
        .get(By::new().predicate(|node| node.author_id() == Some("activity.archive.search")))
        .type_text("Calendar fixture");
    harness.run();
    harness
        .get(By::new().predicate(|node| node.author_id() == Some("activity.2")))
        .click();
    harness.run();
    assert_eq!(
        *harness.state(),
        2,
        "archive ordering must not replace the original index"
    );
}

#[test]
#[ignore = "renders calendar hover states with a headless GPU"]
fn capture_calendar_hover() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/calendar-hover");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("light", egui::ThemePreference::Light),
        ("dark", egui::ThemePreference::Dark),
    ] {
        let mut harness = harness();
        harness.run();
        harness.ctx.set_theme(theme);
        harness.run();
        for state in ["rest", "selected", "unselected"] {
            if state != "rest" {
                harness
                    .get(By::new().predicate(|node| {
                        node.author_id()
                            .is_some_and(|id| id.starts_with("activity.calendar.1970-"))
                            && !node.is_disabled()
                            && node.value().as_deref() == Some(state)
                    }))
                    .hover();
                harness.run();
            }
            harness
                .render()?
                .save(out.join(format!("{name}-{state}.png")))?;
        }
    }
    Ok(())
}
