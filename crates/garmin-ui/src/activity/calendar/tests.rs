use super::*;
use garmin_i18n::{Language, Translations};
use garmin_model::{activity::ActivitySport, identity::UnitSystem};
use jiff::tz::TimeZone;

fn fixture(instant: &str, zone: &TimeZone) -> Presentation {
    use garmin_model::activity::{
        ActivityDuration, ActivityMetrics, ActivitySummary, ActivityTotals, TimeRange,
    };
    let instant: jiff::Timestamp = instant.parse().unwrap();
    let time =
        garmin_model::value::Timestamp::from_unix_milliseconds(instant.as_millisecond()).unwrap();
    let duration = ActivityDuration::from_milliseconds(0);
    let summary = ActivitySummary::from_parts(
        ActivitySport::Cycling,
        TimeRange::from_parts(time, time).unwrap(),
        ActivityTotals::from_parts(duration, duration, None, None, None, None).unwrap(),
        ActivityMetrics::default(),
    );
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let mut presentation =
        Presentation::from_summary(summary, "Fixture", &intl, UnitSystem::Metric);
    presentation.local_start = instant.to_zoned(zone.clone()).datetime();
    presentation
}

#[test]
fn six_week_grid_handles_leap_days_and_locale_week_starts() {
    let translations = Translations::bundled().unwrap();
    let monday = translations
        .formatter_with_locale(Language::English, "en-GB")
        .unwrap();
    let sunday = translations
        .formatter_with_locale(Language::English, "en-US")
        .unwrap();
    let february = monday
        .dates()
        .month(Date::new(2024, 2, 1).unwrap())
        .days
        .map(|cell| cell.map(|cell| cell.date));
    assert!(february[..3].iter().all(Option::is_none));
    assert_eq!(february[31], Some(Date::new(2024, 2, 29).unwrap()));
    assert!(february[32..].iter().all(Option::is_none));
    let march = Date::new(2026, 3, 1).unwrap();
    assert_eq!(
        sunday.dates().month(march).days[0].as_ref().unwrap().date,
        march
    );
    let monday = monday
        .dates()
        .month(march)
        .days
        .map(|cell| cell.map(|cell| cell.date));
    assert_eq!(monday[6], Some(march));
    assert_eq!(monday[36], Some(Date::new(2026, 3, 31).unwrap()));
}

#[test]
fn day_hover_and_cursors_distinguish_clickable_and_empty_dates() {
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let activities = [fixture("2026-09-09T12:00:00Z", &TimeZone::UTC)];
    let month = intl.dates().month(Date::new(2026, 9, 1).unwrap());
    for theme in [egui::ThemePreference::Light, egui::ThemePreference::Dark] {
        for (date, selected, expected_cursor) in [
            (9, None, egui::CursorIcon::PointingHand),
            (9, Some(0), egui::CursorIcon::PointingHand),
            (10, Some(0), egui::CursorIcon::NotAllowed),
        ] {
            let context = egui::Context::default();
            crate::install(&context);
            context.set_theme(theme);
            let day = month
                .days
                .iter()
                .flatten()
                .find(|cell| cell.date.day() == date)
                .unwrap();
            let mut bounds = egui::Rect::NOTHING;
            let mut fills = Vec::new();
            for hovering in [false, true, false] {
                let pointer = if hovering {
                    bounds.center()
                } else {
                    egui::pos2(90.0, 90.0)
                };
                let output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(100.0, 100.0),
                        )),
                        events: vec![egui::Event::PointerMoved(pointer)],
                        ..egui::RawInput::default()
                    },
                    |ui| {
                        bounds = ui
                            .scope(|ui| {
                                day_button(
                                    ui,
                                    day,
                                    selected.map(|_| activities[0].local_start.date()),
                                    &activities,
                                    selected,
                                    36.0,
                                );
                            })
                            .response
                            .rect;
                    },
                );
                let cursor = output.platform_output.cursor_icon;
                let fill = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.rect == bounds && rect.fill.is_opaque() => {
                        Some(rect.fill)
                    }
                    _ => None,
                });
                fills.push(fill.expect("calendar tile background"));
                output.drop_without_applying_deltas();
                assert_eq!(
                    cursor,
                    if hovering {
                        expected_cursor
                    } else {
                        egui::CursorIcon::Default
                    }
                );
            }
            assert_eq!(fills[0], fills[2], "pointer exit restores the resting fill");
            if expected_cursor == egui::CursorIcon::PointingHand {
                assert!(
                    fills[0].r().abs_diff(fills[1].r()) >= 12,
                    "clickable days need visible hover contrast, including the selected day"
                );
            } else {
                assert_eq!(fills[0], fills[1], "empty days must not look interactive");
            }
        }
    }
}

#[test]
fn empty_days_are_disabled_and_current_day_preserves_selection() {
    let zone = TimeZone::UTC;
    let activities = [
        fixture("2026-01-01T16:00:00Z", &zone),
        fixture("2026-01-01T08:00:00Z", &zone),
        fixture("2025-12-30T09:00:00Z", &zone),
    ];
    let day = Date::new(2026, 1, 1).unwrap();
    assert_eq!(select_day(&activities, Some(2), day), Some(1));
    assert_eq!(select_day(&activities, Some(0), day), Some(0));
    assert_eq!(
        select_day(&activities, None, Date::new(2025, 12, 31).unwrap()),
        None
    );
    assert_eq!(chronological(&activities), [2, 1, 0]);
    let mut calendar = Calendar::default();
    calendar.sync(&activities, Some(2));
    assert_eq!(calendar.month, Some(Date::new(2025, 12, 30).unwrap()));
    calendar.sync(&activities, Some(0));
    assert_eq!(calendar.month, Some(Date::new(2026, 1, 1).unwrap()));
    calendar.month = Some(Date::new(2026, 2, 1).unwrap());
    calendar.sync(&activities, Some(0));
    assert_eq!(
        calendar.month,
        Some(Date::new(2026, 2, 1).unwrap()),
        "browsing a month must not change selection"
    );
}

#[test]
fn local_midnight_and_repeated_dst_hour_preserve_instant_order() {
    let zone = TimeZone::get("Europe/Helsinki").unwrap();
    let activities = [
        fixture("2026-10-25T01:30:00Z", &zone),
        fixture("2026-10-25T00:30:00Z", &zone),
        fixture("2026-10-24T21:30:00Z", &zone),
    ];
    assert_eq!(
        activities[0].local_start.time(),
        activities[1].local_start.time()
    );
    assert_eq!(
        on_day(&activities, Date::new(2026, 10, 25).unwrap()),
        [2, 1, 0]
    );
    let english = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let czech = Translations::bundled()
        .unwrap()
        .formatter(Language::Czech)
        .unwrap();
    let local = activities[2].local_start;
    assert!(date_time_label(&english, local).contains("25"));
    assert!(date_time_label(&czech, local).contains("25"));
}

#[test]
fn spring_gap_and_skipped_civil_day_use_the_timezone_database() {
    let helsinki = TimeZone::get("Europe/Helsinki").unwrap();
    let before = fixture("2026-03-29T00:30:00Z", &helsinki);
    let after = fixture("2026-03-29T01:30:00Z", &helsinki);
    assert_eq!(before.local_start.hour(), 2);
    assert_eq!(after.local_start.hour(), 4);
    assert_eq!(before.local_start.date(), after.local_start.date());

    let apia = TimeZone::get("Pacific/Apia").unwrap();
    let activities = [
        fixture("2011-12-30T09:30:00Z", &apia),
        fixture("2011-12-30T10:30:00Z", &apia),
    ];
    assert_eq!(
        activities[0].local_start.date(),
        jiff::civil::date(2011, 12, 29)
    );
    assert_eq!(
        activities[1].local_start.date(),
        jiff::civil::date(2011, 12, 31)
    );
    assert!(on_day(&activities, jiff::civil::date(2011, 12, 30)).is_empty());
    assert_eq!(chronological(&activities), [0, 1]);
}
