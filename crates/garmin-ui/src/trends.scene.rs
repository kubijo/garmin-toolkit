use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::{
    activity::{ActivityDuration, ActivitySport, Distance, HeartRate},
    garmin_connect::{
        CachedActivity, RemoteActivity,
        sync::{Revision, Snapshot},
        trends::{History, ImportedMeasurements, Metric, Period},
    },
    identity::{MonthlyDistanceTarget, ProfilePreferences},
};
use garmin_ui::trends;

scene_meta! { title: "Application / Trends" }

thread_local! { static STATE: crate::SceneState<trends::State, 1536> = const { crate::SceneState::empty() }; }

#[scene(default)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the interactive fixture and its controls together"
)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 1280.0, 320.0, 1440.0, 1.0);
    let height = ctx.slider("height", 1600.0, 480.0, 4000.0, 1.0);
    let variant = ctx.buttons("history", &["cached", "empty", "missing", "no cache"], 0);
    let goals = ctx.toggle("targets", true);
    let partial_goals = ctx.toggle("partial targets", false);
    let period = ctx.buttons("period", &["day", "week", "month", "year"], 2);
    let status = ctx.buttons("status", &["ready", "running", "failed read", "offline"], 0);
    let previous = ctx.toggle("comparison history", true);
    let imperial = ctx.toggle("imperial", false);
    let cumulative = ctx.toggle("cumulative", false);
    let editor = ctx.toggle("edit targets", false);
    let metric = ctx.buttons(
        "comparison metric",
        &["distance", "elevation", "heart rate"],
        0,
    );
    let unavailable = ctx.toggle("missing original", false);
    let intl = globals.intl();
    let history = if variant == 3 {
        None
    } else {
        Some(fixture(variant, previous))
    };
    stage!(ctx, ui, globals.stage((width, height)), |ui| {
        STATE.with_scene(
            variant
                + 4 * period
                + 16 * status
                + 64 * usize::from(previous)
                + 128 * usize::from(cumulative)
                + 256 * usize::from(editor)
                + 512 * metric,
            || {
                let mut state = trends::State::from_cache(history, Revision::default())
                    .with_presentation(garmin_ui::modal::Presentation::Contained)
                    .with_cumulative(cumulative)
                    .with_period([Period::Day, Period::Week, Period::Month, Period::Year][period])
                    .with_comparison_metric(
                        [
                            Metric::Distance,
                            Metric::Elevation,
                            Metric::AverageHeartRate,
                        ][metric],
                    );
                if status >= 2 {
                    state.complete(
                        &Revision::default(),
                        Err(if status == 3 {
                            "Connection unavailable"
                        } else {
                            "Fixture cache read failed"
                        }
                        .to_owned()),
                    );
                }
                if editor {
                    state.edit_targets([0, 200, 0].map(MonthlyDistanceTarget::from_kilometres));
                }
                state
            },
            |state| {
                let preferences = ProfilePreferences::from_parts(
                    if imperial {
                        garmin_model::identity::UnitSystem::Imperial
                    } else {
                        garmin_model::identity::UnitSystem::Metric
                    },
                    garmin_model::identity::LanguagePreference::English,
                    garmin_model::identity::ThemePreference::Auto,
                )
                .with_monthly_distance_targets(
                    (if partial_goals {
                        [0, 200, 0]
                    } else if goals {
                        [120, 750, 50]
                    } else {
                        [0; 3]
                    })
                    .map(MonthlyDistanceTarget::from_kilometres),
                );
                let mut sync = Snapshot {
                    progress: (status == 1).then_some(
                        garmin_model::garmin_connect::LoadProgress::Indexing {
                            discovered: 80,
                            pages: 4,
                        },
                    ),
                    ..Default::default()
                };
                if unavailable {
                    sync.state.outcome =
                        garmin_model::garmin_connect::sync::Outcome::Complete { unavailable: 1 };
                }
                let _ = trends::show(
                    ui,
                    state,
                    &trends::Props {
                        intl: &intl,
                        today: "2026-11-16".parse().expect("fixture date"),
                        preferences,
                        sync: Some(&sync),
                        connect_configured: true,
                        enabled: status != 3,
                        refreshing: status == 1,
                        load_progress: None,
                    },
                );
            },
        );
    });
}

type Sample = (u8, ActivitySport, f64, u32);

fn fixture(variant: usize, previous: bool) -> History {
    if variant == 1 {
        return History::default();
    }
    let mut summaries = november();
    summaries.extend(earlier_months());
    if previous {
        summaries.extend(october());
        let prior_year = summaries
            .iter()
            .cloned()
            .map(|mut summary| {
                summary.activity.id -= 100_000;
                summary.activity.distance = summary
                    .activity
                    .distance
                    .and_then(|value| Distance::try_from_metres(value.as_metres() * 0.82));
                summary.activity.duration = summary
                    .activity
                    .duration
                    .and_then(|value| ActivityDuration::try_from_seconds(value.as_seconds() * 0.9));
                summary.started_local = summary
                    .started_local
                    .map(|date| date.replace("2026-", "2025-"));
                summary
            })
            .collect::<Vec<_>>();
        summaries.extend(prior_year);
    }
    if variant == 2 {
        for summary in &mut summaries {
            match summary.started_local.as_deref() {
                Some("2026-11-05T06:00:00") => summary.activity.distance = None,
                Some("2026-11-10T16:00:00") => summary.activity.duration = None,
                _ => {}
            }
        }
    }
    let imported = summaries
        .iter()
        .enumerate()
        .map(|(index, summary)| {
            let metres = if summary.activity.sport == Some(ActivitySport::Swimming) {
                summary
                    .started_local
                    .as_deref()
                    .is_some_and(|date| date.starts_with("2026-11-16"))
                    .then_some(0)
            } else if index.is_multiple_of(3) {
                None
            } else {
                Some(20 + u64::try_from(index % 12).expect("bounded index") * 25)
            };
            (
                summary.activity.id,
                ImportedMeasurements {
                    ascent: metres.map(|value| Distance::from_millimeters(value * 1_000)),
                    average_heart_rate: (!index.is_multiple_of(4)).then(|| {
                        HeartRate::from_beats_per_minute(
                            118 + u16::try_from(index % 40).expect("bounded index"),
                        )
                    }),
                },
            )
        })
        .collect();
    History::from_summaries_with_measurements(&summaries, &imported)
}

fn november() -> Vec<CachedActivity> {
    use ActivitySport::{Cycling as Ride, Running as Run, Swimming as Swim};
    // Rest days on the 6th and 13th; one to three activities on the other days.
    // A recorded pool-drill session has a known zero distance.
    samples(
        2026,
        11,
        [
            (1, Run, 6.0, 35),
            (1, Ride, 28.0, 65),
            (2, Swim, 1.2, 32),
            (3, Run, 10.0, 54),
            (4, Ride, 52.0, 112),
            (4, Swim, 1.8, 43),
            (5, Run, 5.0, 28),
            (5, Ride, 18.0, 42),
            (5, Swim, 0.0, 12),
            (7, Run, 16.0, 94),
            (7, Ride, 86.0, 190),
            (8, Swim, 2.0, 48),
            (9, Run, 7.5, 43),
            (9, Ride, 34.0, 78),
            (10, Ride, 65.0, 140),
            (11, Run, 12.0, 68),
            (11, Swim, 2.5, 59),
            (12, Ride, 22.0, 51),
            (12, Swim, 1.5, 37),
            (14, Run, 8.5, 49),
            (14, Ride, 47.0, 101),
            (15, Swim, 3.0, 72),
            (16, Run, 13.0, 76),
            (16, Ride, 88.0, 191),
            (16, Swim, 4.0, 95),
        ],
    )
}

fn october() -> Vec<CachedActivity> {
    use ActivitySport::{Cycling as Ride, Running as Run, Swimming as Swim};
    // Independent dates and counts make comparisons more than a scaled copy.
    samples(
        2026,
        10,
        [
            (1, Run, 8.0, 50),
            (2, Ride, 40.0, 100),
            (3, Swim, 1.4, 35),
            (3, Run, 6.0, 38),
            (6, Ride, 60.0, 140),
            (7, Run, 12.0, 73),
            (7, Swim, 2.0, 50),
            (9, Ride, 50.0, 115),
            (10, Run, 5.0, 32),
            (12, Ride, 70.0, 160),
            (12, Swim, 1.6, 42),
            (14, Run, 10.0, 60),
            (16, Ride, 45.0, 104),
            (16, Swim, 2.5, 65),
            (18, Run, 8.0, 47),
            (21, Ride, 52.0, 119),
            (23, Swim, 2.0, 49),
            (25, Run, 14.0, 82),
            (28, Ride, 75.0, 168),
            (31, Swim, 2.5, 61),
        ],
    )
}

fn earlier_months() -> Vec<CachedActivity> {
    use ActivitySport::{Cycling, Running, Swimming};
    let months = [
        (1, [55.0, 180.0, 8.0]),
        (3, [70.0, 260.0, 10.0]),
        (4, [90.0, 330.0, 12.0]),
        (5, [105.0, 420.0, 15.0]),
        (6, [60.0, 510.0, 18.0]),
        (7, [125.0, 620.0, 20.0]),
        (8, [95.0, 480.0, 14.0]),
        (9, [82.0, 390.0, 9.5]),
    ];
    let mut history = Vec::new();
    for (month, distances) in months {
        let mut sessions = Vec::new();
        for (index, (sport, minutes_per_km)) in [(Running, 5.8), (Cycling, 2.3), (Swimming, 25.0)]
            .into_iter()
            .enumerate()
        {
            for (session, weight) in [0.7, 1.0, 1.3, 0.8, 1.6, 0.6].into_iter().enumerate() {
                let day = u8::try_from(1 + (session * 5 + index * 3) % 28).expect("fixture day");
                let distance = distances[index] * weight / 6.0;
                let duration = std::time::Duration::from_secs_f64(distance * minutes_per_km * 60.0);
                let minutes =
                    u32::try_from(duration.as_secs().div_ceil(60)).expect("fixture duration");
                sessions.push((day, sport, distance, minutes));
            }
        }
        history.extend(samples(2026, month, sessions));
    }
    history
}

fn samples(year: i32, month: u8, entries: impl IntoIterator<Item = Sample>) -> Vec<CachedActivity> {
    entries
        .into_iter()
        .enumerate()
        .map(|(index, (day, sport, distance, minutes))| {
            let hour = 6 + (index % 3) * 5;
            CachedActivity {
                activity: RemoteActivity {
                    id: i64::from(year) * 100_000
                        + i64::from(month) * 1000
                        + i64::try_from(index).expect("fixture index")
                        + 1,
                    imported: index.is_multiple_of(4),
                    original_unavailable: false,
                    sport: Some(sport),
                    name: match sport {
                        ActivitySport::Running => "Training run",
                        ActivitySport::Cycling => "Road ride",
                        ActivitySport::Swimming => "Pool swim",
                        ActivitySport::Walking => "Evening walk",
                        ActivitySport::Hiking => "Trail hike",
                    }
                    .to_owned(),
                    started_at: None,
                    distance: Distance::try_from_kilometres(distance),
                    duration: ActivityDuration::try_from_seconds(f64::from(minutes) * 60.0),
                },
                description: None,
                activity_type: None,
                started_local: Some(format!("{year}-{month:02}-{day:02}T{hour:02}:00:00")),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_model::garmin_connect::trends::{BucketValue, Range, Sport};

    #[test]
    fn fixture_exercises_rest_days_counts_heat_levels_and_real_period_comparisons() {
        let history = fixture(0, true);
        let range = Range {
            from: "2026-11-01".parse().expect("date"),
            through: "2026-11-16".parse().expect("date"),
        };
        assert_eq!(history.totals(range, None).activities, 25);
        assert_eq!(history.active_days(range), 14);
        let counts = history.series(range, None, Metric::Count, false);
        for count in 0..=3 {
            assert!(
                counts
                    .iter()
                    .any(|day| day.value == Some(BucketValue::Count(count)))
            );
        }
        let durations = history.series(range, None, Metric::Duration, false);
        assert!(
            durations
                .iter()
                .any(|day| matches!(day.value, Some(BucketValue::Duration(time))
                    if time > ActivityDuration::default() && time < ActivityDuration::from_milliseconds(3_600_000)))
        );
        assert!(
            durations
                .iter()
                .any(|day| matches!(day.value, Some(BucketValue::Duration(time))
                    if time > ActivityDuration::from_milliseconds(7_200_000)))
        );
        let swimming = history.series(range, Some(Sport::Swimming), Metric::Distance, false);
        assert_eq!(
            swimming[4].value,
            Some(BucketValue::Distance(Distance::default()))
        );
        let previous = Range {
            from: "2026-10-01".parse().expect("date"),
            through: "2026-10-16".parse().expect("date"),
        };
        assert_eq!(history.totals(previous, None).activities, 14);
        assert_eq!(history.active_days(previous), 10);
        let year = Range {
            from: "2026-01-01".parse().expect("date"),
            through: range.through,
        };
        let months = history.series(year, None, Metric::Count, true);
        assert_eq!(months[1].value, Some(BucketValue::Count(0)));
        assert!(matches!(months[0].value, Some(BucketValue::Count(count)) if count > 0));
    }

    #[test]
    fn missing_fields_retain_activity_counts_and_healthy_sport_data() {
        let complete = fixture(0, true);
        let missing = fixture(2, true);
        let range = Range {
            from: "2026-11-01".parse().expect("date"),
            through: "2026-11-16".parse().expect("date"),
        };
        let totals = missing.totals(range, None);
        assert_eq!(totals.activities, complete.totals(range, None).activities);
        assert_eq!(totals.missing_distance, 1);
        assert_eq!(totals.missing_duration, 1);
        assert_eq!(
            missing
                .totals(range, Some(Sport::Swimming))
                .missing_distance,
            0
        );
    }
}
