use super::*;
use crate::garmin_connect::RemoteActivity;

fn date(text: &str) -> Date {
    text.parse().expect("valid fixture date")
}

fn summary(
    id: i64,
    local: Option<&str>,
    distance: Option<f64>,
    duration: Option<f64>,
) -> CachedActivity {
    CachedActivity {
        activity: RemoteActivity {
            id,
            imported: false,
            original_unavailable: false,
            sport: Some(ActivitySport::Running),
            name: String::new(),
            started_at: Some("2026-11-15T23:30:00Z".into()),
            distance: distance.and_then(Distance::try_from_metres),
            duration: duration.and_then(ActivityDuration::try_from_seconds),
        },
        description: None,
        activity_type: None,
        started_local: local.map(str::to_owned),
    }
}

#[test]
fn local_dates_missing_fields_and_duplicates_are_not_silent_zeroes() {
    let mut imported = summary(1, Some("2026-11-16 01:30:00"), Some(10_000.0), Some(3600.0));
    imported.activity.imported = true;
    let history = History::from_summaries(&[
        imported.clone(),
        imported,
        summary(2, Some("2026-11-16T08:00:00"), None, Some(60.0)),
        summary(3, Some("2026-11-16T12:00:00"), Some(f64::NAN), Some(-1.0)),
        summary(4, None, Some(100.0), Some(60.0)),
    ]);
    let range = Range {
        from: date("2026-11-16"),
        through: date("2026-11-16"),
    };
    let totals = history.totals(range, None);
    assert_eq!(history.summaries, 4);
    assert_eq!(history.undated, 1);
    assert_eq!(
        history
            .activities
            .iter()
            .map(|activity| activity.id)
            .collect::<Vec<_>>(),
        [3, 2, 1]
    );
    assert_eq!(history.activities[0].distance, None);
    assert_eq!(history.activities[0].duration, None);
    assert_eq!(totals.activities, 3);
    assert_eq!(totals.distance.as_metres(), 10_000.0);
    assert_eq!(totals.duration.as_seconds(), 3660.0);
    assert_eq!(totals.missing_distance, 2);
    assert_eq!(totals.missing_duration, 1);
    assert_eq!(history.active_days(range), 1);
}

#[test]
fn elevation_tracks_coverage_including_reported_zero_and_leaves_incomplete_buckets_unknown() {
    let summaries = [
        summary(1, Some("2026-11-01"), None, None),
        summary(2, Some("2026-11-02"), None, None),
        summary(3, Some("2026-11-03"), None, None),
    ];
    let history = History::from_summaries_with_measurements(
        &summaries,
        &BTreeMap::from([
            (
                1,
                ImportedMeasurements {
                    ascent: Some(Distance::from_millimeters(420_000)),
                    ..Default::default()
                },
            ),
            (
                2,
                ImportedMeasurements {
                    ascent: Some(Distance::default()),
                    ..Default::default()
                },
            ),
        ]),
    );
    let range = Range {
        from: date("2026-11-01"),
        through: date("2026-11-03"),
    };
    let totals = history.totals(range, None);
    assert_eq!(totals.elevation_gain, Distance::from_millimeters(420_000));
    assert_eq!(totals.elevation_activities, 2);
    assert_eq!(totals.activities, 3);
    assert_eq!(
        history
            .series(range, None, Metric::Elevation, false)
            .into_iter()
            .map(|bucket| bucket.value)
            .collect::<Vec<_>>(),
        [
            Some(BucketValue::Distance(Distance::from_millimeters(420_000))),
            Some(BucketValue::Distance(Distance::default())),
            None,
        ],
    );
}

#[test]
fn heart_rate_counts_only_reported_activities_and_averages_them_across_days() {
    let summaries = [
        summary(1, Some("2026-11-01"), None, None),
        summary(2, Some("2026-11-01"), None, None),
        summary(3, Some("2026-11-02"), None, None),
        summary(4, Some("2026-11-03"), None, None),
    ];
    let imported = [(1, 100), (2, 120), (3, 170)]
        .into_iter()
        .map(|(id, bpm)| {
            (
                id,
                ImportedMeasurements {
                    average_heart_rate: Some(HeartRate::from_beats_per_minute(bpm)),
                    ..Default::default()
                },
            )
        })
        .collect();
    let history = History::from_summaries_with_measurements(&summaries, &imported);
    let range = Range {
        from: date("2026-11-01"),
        through: date("2026-11-03"),
    };
    let totals = history.totals(range, None);
    assert_eq!(totals.activities, 4);
    assert_eq!(totals.heart_rate.activities(), 3);
    let bucket = |bpm| {
        Some(BucketValue::HeartRate(HeartRate::from_beats_per_minute(
            bpm,
        )))
    };
    assert_eq!(
        history.series(range, None, Metric::AverageHeartRate, true)[0].value,
        bucket(130)
    );
    assert_eq!(
        history
            .series(range, None, Metric::AverageHeartRate, false)
            .into_iter()
            .map(|item| item.value)
            .collect::<Vec<_>>(),
        [bucket(110), bucket(170), None],
    );
}

#[test]
fn peak_power_keeps_the_highest_measured_ride_and_counts_zero_watts() {
    let mut summaries = Vec::new();
    let mut measurements = BTreeMap::new();
    for (id, day, sport, watts) in [
        (1, "2026-11-01", ActivitySport::Cycling, Some(410)),
        (2, "2026-11-01", ActivitySport::Cycling, Some(230)),
        (3, "2026-11-02", ActivitySport::Cycling, Some(0)),
        (4, "2026-11-03", ActivitySport::Cycling, None),
        (5, "2026-11-03", ActivitySport::Running, Some(600)),
    ] {
        let mut ride = summary(id, Some(day), None, None);
        ride.activity.sport = Some(sport);
        summaries.push(ride);
        measurements.insert(
            id,
            ImportedMeasurements {
                peak_power: watts.map(Power::from_watts),
                ..Default::default()
            },
        );
    }
    let history = History::from_summaries_with_measurements(&summaries, &measurements);
    let range = Range {
        from: date("2026-11-01"),
        through: date("2026-11-04"),
    };
    let cycling = Some(Sport::Cycling);
    let totals = history.totals(range, cycling);
    assert_eq!((totals.power_activities, totals.activities), (3, 4));
    let peak = |watts| Some(BucketValue::Power(Power::from_watts(watts)));
    let series = |monthly| {
        history
            .series(range, cycling, Metric::PeakPower, monthly)
            .into_iter()
            .map(|bucket| bucket.value)
            .collect::<Vec<_>>()
    };
    assert_eq!(series(false), [peak(410), peak(0), None, None]);
    assert_eq!(series(true), [peak(410)]);
    assert_eq!(
        history.totals(range, None).peak_power,
        Some(Power::from_watts(410))
    );
}

#[test]
fn comparisons_match_calendar_dates_and_clamp_short_months() {
    let today = date("2026-11-16");
    let (current, previous) = Period::Month.ranges(today, today).expect("month range");
    assert_eq!(
        current,
        Range {
            from: date("2026-11-01"),
            through: today
        }
    );
    assert_eq!(previous.through, date("2026-10-16"));
    let (_, previous) = Period::Month
        .ranges(date("2024-03-31"), today)
        .expect("leap month");
    assert_eq!(previous.through, date("2024-02-29"));
    let (_, previous) = Period::Year
        .ranges(date("2024-11-16"), date("2024-11-16"))
        .expect("leap year");
    assert_eq!(previous.through, date("2023-11-16"));
}

#[test]
fn weeks_start_on_monday_and_historical_periods_are_complete() {
    let (current, previous) = Period::Week
        .ranges(date("2026-11-17"), date("2026-11-17"))
        .expect("week");
    assert_eq!(current.from, date("2026-11-16"));
    assert_eq!(
        previous,
        Range {
            from: date("2026-11-09"),
            through: date("2026-11-10")
        }
    );
    let (current, _) = Period::Month
        .ranges(date("2026-10-01"), date("2026-11-17"))
        .expect("historical month");
    assert_eq!(current.through, date("2026-10-31"));
    assert_eq!(current.dates().len(), 31);
}

#[test]
fn locale_week_starts_and_unknown_cumulative_samples_stay_consistent() {
    let today = date("2026-11-17");
    let (range, _) = Period::Week
        .ranges_with_week_start(today, today, jiff::civil::Weekday::Sunday)
        .expect("Sunday week");
    assert_eq!(range.from, date("2026-11-15"));
    let history = History::from_summaries(&[
        summary(1, Some("2026-11-15T10:00:00"), Some(1000.0), Some(60.0)),
        summary(2, Some("2026-11-16T10:00:00"), None, Some(60.0)),
        summary(3, Some("2026-11-17T10:00:00"), Some(2000.0), Some(60.0)),
    ]);
    let mut series = history.series(range, None, Metric::Distance, false);
    cumulative(&mut series);
    assert_eq!(
        series.iter().map(|bucket| bucket.value).collect::<Vec<_>>(),
        [
            Some(BucketValue::Distance(Distance::from_millimeters(1_000_000))),
            None,
            None
        ]
    );
    assert_eq!(
        history
            .series(range, None, Metric::Count, false)
            .iter()
            .map(|bucket| bucket.value)
            .collect::<Vec<_>>(),
        [Some(BucketValue::Count(1)); 3]
    );
}
