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
            sport: Some(ActivitySport::Running),
            name: String::new(),
            started_at: Some("2026-11-15T23:30:00Z".into()),
            distance_metres: distance,
            duration_seconds: duration,
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
    assert_eq!(totals.activities, 3);
    assert_eq!(totals.distance_metres, 10_000.0);
    assert_eq!(totals.duration_seconds, 3660.0);
    assert_eq!(totals.missing_distance, 2);
    assert_eq!(totals.missing_duration, 1);
    assert_eq!(history.active_days(range), 1);
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
        [Some(1000.0), None, None]
    );
    assert_eq!(
        history
            .series(range, None, Metric::Count, false)
            .iter()
            .map(|bucket| bucket.value)
            .collect::<Vec<_>>(),
        [Some(1.0); 3]
    );
}
