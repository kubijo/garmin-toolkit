//! Interaction tests exercise the real shared Rust page rather than a prototype renderer.

use gallery::eframe::egui;

use egui_kittest::{
    Harness,
    kittest::{By, Queryable as _},
};
use garmin_model::{
    garmin_connect::{
        sync::{Outcome, Revision, Snapshot},
        trends::History,
    },
    identity::ProfilePreferences,
};
use garmin_ui::trends::{Action, Props, State};

struct Model {
    state: State,
    preferences: ProfilePreferences,
    reads: usize,
    syncs: usize,
    sync: Option<Snapshot>,
    cloud_opens: usize,
}

fn harness() -> Harness<'static, Model> {
    let intl = garmin_i18n::Translations::bundled()
        .expect("catalog")
        .formatter(garmin_i18n::Language::English)
        .expect("locale");
    let mut installed = false;
    Harness::builder()
        .with_size([760.0, 2400.0])
        .with_max_steps(32)
        .build_ui_state(
            move |ui, model: &mut Model| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                if let Some(action) = garmin_ui::trends::show(
                    ui,
                    &mut model.state,
                    &Props {
                        intl: &intl,
                        today: "2026-11-17".parse().expect("date"),
                        preferences: model.preferences,
                        sync: model.sync.as_ref(),
                        enabled: true,
                        refreshing: false,
                        load_progress: None,
                    },
                ) {
                    match action {
                        Action::Read(revision) => {
                            model.reads += 1;
                            model
                                .state
                                .complete(&revision, Ok(Some(History::default())));
                        }
                        Action::Sync => model.syncs += 1,
                        Action::Cancel(_) => {}
                        Action::OpenActivities => model.cloud_opens += 1,
                        Action::SaveTargets(targets) => {
                            model.preferences =
                                model.preferences.with_monthly_distance_targets(targets);
                        }
                    }
                }
            },
            Model {
                state: State::default(),
                preferences: ProfilePreferences::default(),
                reads: 0,
                syncs: 0,
                sync: None,
                cloud_opens: 0,
            },
        )
}

#[test]
fn browsing_periods_and_targets_never_starts_a_cloud_sync() {
    let mut view = harness();
    view.run();
    assert_eq!(view.state().reads, 1);
    view.get(By::new().predicate(|node| node.author_id() == Some("trends.period.0")))
        .click();
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("trends.period.3")))
        .click();
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("trends.previous")))
        .click();
    view.run();
    assert_eq!(view.state().reads, 1);
    assert_eq!(view.state().syncs, 0);
}

#[test]
fn custom_dates_apply_locally_and_return_to_a_preset() {
    let mut view = harness();
    let from = || By::new().predicate(|node| node.author_id() == Some("trends.range.from"));
    let active = || By::new().predicate(|node| node.author_id() == Some("trends.range.active"));
    view.run();
    view.get_by_label("Custom").click();
    view.run();
    view.get(from()).click();
    view.run();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(from()).type_text("2026-11-10");
    view.run();
    assert_eq!(view.get(from()).value().as_deref(), Some("2026-11-10"));
    view.get_by_label("Apply date range").click();
    view.run();
    assert!(
        view.query_by_label("Edit date range").is_some(),
        "range was not applied"
    );
    assert_eq!(
        view.get(active()).value().as_deref(),
        Some("2026-11-10/2026-11-17")
    );
    view.get_by_label("Edit date range").click();
    view.run();
    view.get_by_label("Cancel").click();
    view.run();
    assert_eq!(
        view.get(active()).value().as_deref(),
        Some("2026-11-10/2026-11-17")
    );
    view.get_by_label("Month").click();
    view.run();
    assert_eq!(
        view.get(active()).value().as_deref(),
        Some("2026-11-01/2026-11-17")
    );
    assert_eq!(view.state().reads, 1);
    assert_eq!(view.state().syncs, 0);
}

#[test]
fn published_empty_history_is_not_a_missing_cache_and_reload_is_explicit() {
    let mut view = harness();
    view.run();
    view.get_by_label("Reload cached history").click();
    view.run();
    assert_eq!(view.state().reads, 2);
    assert_eq!(view.state().syncs, 0);
    view.state_mut().state = State::from_cache(None, Revision::default());
    view.run();
    view.get_by_label("Sync activities").click();
    view.run();
    assert_eq!(view.state().syncs, 1);
}

#[test]
fn target_editor_saves_only_on_confirmation_and_cancel_discards_the_draft() {
    let mut view = harness();
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("trends.targets.edit")))
        .click();
    view.run();
    let running = || By::new().predicate(|node| node.author_id() == Some("trends.target.running"));
    view.get(running()).click();
    view.run();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(running()).type_text("120");
    view.run();
    assert_eq!(view.state().preferences.monthly_distance_targets(), [0; 3]);
    view.get(By::new().predicate(|node| node.author_id() == Some("modal.primary")))
        .click();
    view.run();
    assert_eq!(
        view.state().preferences.monthly_distance_targets(),
        [120, 0, 0]
    );
    view.get_by_label("Edit monthly distance targets").click();
    view.run();
    view.get(running()).click();
    view.run();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(running()).type_text("999");
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("modal.cancel")))
        .click();
    view.run();
    assert_eq!(
        view.state().preferences.monthly_distance_targets(),
        [120, 0, 0]
    );
    assert_eq!(view.state().syncs, 0);
    assert_eq!(view.state().reads, 1);
}

#[test]
fn calendar_click_pins_the_date_without_reloading_or_syncing() {
    let mut view = harness();
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("trends.calendar.2026-11-15")))
        .click();
    view.run();
    assert!(
        view.query_by_label("Select a day to pin its totals here.")
            .is_none()
    );
    view.get_by_label("Next").hover();
    view.run();
    assert!(
        view.query_by_label("Select a day to pin its totals here.")
            .is_none()
    );
    assert_eq!(view.state().reads, 1);
    assert_eq!(view.state().syncs, 0);
}

#[test]
fn missing_original_is_informational_and_links_to_cloud_activities() {
    let mut view = harness();
    view.run();
    let mut snapshot = Snapshot::default();
    snapshot.state.outcome = Outcome::Complete { unavailable: 1 };
    view.state_mut().sync = Some(snapshot);
    view.run();
    assert!(
        view.query_by_label("Sync completed with unavailable files")
            .is_none()
    );
    view.get_by_label("View cloud activities").click();
    view.run();
    assert_eq!(view.state().cloud_opens, 1);
    assert_eq!(view.state().syncs, 0);
    assert_eq!(view.state().reads, 1);
}

#[test]
fn calendar_uses_only_the_weeks_needed_to_contain_the_month() {
    let mut view = harness();
    view.run();
    // November 2026 needs five weeks with the English Sunday-first calendar.
    assert!(view.query_by_label("November 30, 2026").is_some());
    assert!(view.query_by_label("December 5, 2026").is_some());
    assert!(view.query_by_label("December 6, 2026").is_none());
    for _ in 0..3 {
        view.get_by_label("Previous").click();
        view.run();
    }
    // August needs all six weeks.
    assert!(view.query_by_label("August 31, 2026").is_some());
    assert!(view.query_by_label("September 5, 2026").is_some());
    assert!(view.query_by_label("September 6, 2026").is_none());
    for _ in 0..6 {
        view.get_by_label("Previous").click();
        view.run();
    }
    // February fits exactly into four weeks.
    assert!(view.query_by_label("February 28, 2026").is_some());
    assert!(view.query_by_label("March 1, 2026").is_none());
    assert_eq!(view.state().reads, 1);
    assert_eq!(view.state().syncs, 0);
}

#[test]
fn year_view_labels_monthly_buckets_and_keeps_long_duration_on_one_line() {
    let mut view = harness();
    view.run();
    view.state_mut().state = State::from_cache(
        Some(History {
            days: vec![garmin_model::garmin_connect::trends::Day {
                date: "2026-11-16".parse().expect("date"),
                sport: garmin_model::garmin_connect::trends::Sport::Running,
                totals: garmin_model::garmin_connect::trends::Totals {
                    activities: 1,
                    duration_seconds: 1_045_380.0,
                    ..Default::default()
                },
            }],
            summaries: 1,
            undated: 0,
        }),
        Revision::default(),
    );
    view.get_by_label("Year").click();
    view.run();
    assert!(view.query_by_label("Monthly").is_some());
    assert!(view.query_by_label("Daily").is_none());
    let duration = view.get_by_label("290h 23m").rect();
    let count = view.get_by_label("1").rect();
    assert!(
        (duration.height() - count.height()).abs() < 1.0,
        "duration must use the same single-line height as the count: {duration:?} versus {count:?}"
    );
}
