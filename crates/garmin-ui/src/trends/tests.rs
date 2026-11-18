use super::*;

#[test]
fn distance_formats_si_and_imperial_using_locale() {
    use garmin_i18n::{Language, Translations};

    let intl = Translations::bundled()
        .expect("catalog")
        .formatter(Language::Czech)
        .expect("locale");
    let decimal = intl.decimal_formatter();
    assert_eq!(
        Distance::from_millimeters(900_000).format(UnitSystem::Metric, 2, &decimal),
        "900\u{a0}m"
    );
    assert_eq!(
        Distance::from_millimeters(1_500_000).format(UnitSystem::Metric, 2, &decimal),
        "1,50\u{a0}km"
    );
    assert_eq!(
        Distance::from_millimeters(1_609_344).format(UnitSystem::Imperial, 2, decimal),
        "1,00\u{a0}mi"
    );
}
use garmin_model::activity::ActivityDuration;

#[test]
fn comparison_starts_on_an_active_sport_and_preserves_later_choices() {
    let date: Date = "2026-11-16".parse().expect("date");
    let range = Range {
        from: date,
        through: date,
    };
    let mut state = State::default();
    state.initialize_comparison(range);
    assert!(
        !state.comparison_initialized,
        "wait for the asynchronous cache read"
    );
    state.history = Some(History {
        days: vec![garmin_model::garmin_connect::trends::Day {
            date,
            sport: Sport::Cycling,
            totals: garmin_model::garmin_connect::trends::Totals {
                activities: 1,
                missing_distance: 1,
                ..Default::default()
            },
        }],
        summaries: 1,
        activities: Vec::new(),
        undated: 0,
    });
    state.initialize_comparison(range);
    assert_eq!(
        Sport::ALL[state.sport],
        Sport::Cycling,
        "missing distance still counts as activity"
    );
    state.sport = 0;
    state.initialize_comparison(range);
    assert_eq!(
        Sport::ALL[state.sport],
        Sport::Running,
        "keep an explicit choice even without activity"
    );
}

#[test]
fn generation_changes_clear_old_reports_even_when_the_next_read_fails() {
    for generation in [None, Some("new-account".to_owned())] {
        let old = Revision {
            generation: Some("old-account".to_owned()),
            completed: None,
        };
        let new = Revision {
            generation,
            completed: None,
        };
        let mut state = State::from_cache(Some(History::default()), old.clone());
        state.selected = Some("2026-11-15".parse().expect("date"));
        assert!(state.needs_read(&new));
        assert!(state.history.is_none());
        assert!(state.selected.is_none());
        state.complete(&old, Ok(Some(History::default())));
        assert!(
            state.history.is_none(),
            "late old-account responses stay discarded"
        );
        state.complete(&new, Err("cache read failed".to_owned()));
        assert!(state.history.is_none());
    }
}

#[test]
fn same_generation_reload_failure_keeps_the_cached_report() {
    let revision = Revision {
        generation: Some("same-account".to_owned()),
        completed: None,
    };
    let mut state = State::from_cache(Some(History::default()), revision.clone());
    state.read = ReadState::Idle;
    assert!(state.needs_read(&revision));
    state.complete(&revision, Err("offline".to_owned()));
    assert!(state.history.is_some());
    let completed = Revision {
        completed: Some(garmin_model::value::Timestamp::from_unix_seconds(1).expect("instant")),
        ..revision
    };
    assert!(state.needs_read(&completed));
    assert!(state.history.is_some());
    state.revision = Some(completed.clone());
    state.complete(&completed, Err("read failed".to_owned()));
    assert!(state.history.is_some());
}

#[test]
fn superseded_cache_reads_cannot_replace_a_new_generation() {
    let old = Revision {
        generation: Some("old".into()),
        completed: None,
    };
    let new = Revision {
        generation: Some("new".into()),
        completed: None,
    };
    let mut state = State::from_cache(Some(History::default()), new.clone());
    state.read = ReadState::Loading;
    state.complete(&old, Err("stale error".into()));
    assert!(matches!(state.read, ReadState::Loading));
    state.complete(&new, Err("offline".into()));
    assert!(matches!(state.read, ReadState::Failed(_)));
    assert!(
        state.history.is_some(),
        "read failures retain published history"
    );
}

#[test]
fn day_year_and_missing_measurement_views_keep_integer_counts() {
    let history = History {
        days: vec![garmin_model::garmin_connect::trends::Day {
            date: "2026-11-16".parse().expect("date"),
            sport: Sport::Running,
            totals: garmin_model::garmin_connect::trends::Totals {
                activities: 2,
                missing_distance: 1,
                duration: ActivityDuration::try_from_seconds(3600.0).expect("fixture measurement"),
                ..Default::default()
            },
        }],
        activities: Vec::new(),
        undated: 0,
        summaries: 2,
    };
    let date = "2026-11-16".parse().expect("date");
    let range = Range {
        from: date,
        through: date,
    };
    let (_, distance) = values(
        &history,
        range,
        Some(Sport::Running),
        Metric::Distance,
        false,
        true,
        ProfilePreferences::default(),
    );
    let (_, counts) = values(
        &history,
        range,
        Some(Sport::Running),
        Metric::Count,
        false,
        true,
        ProfilePreferences::default(),
    );
    assert_eq!(distance, [None]);
    assert_eq!(counts, [Some(2.0)]);
}
#[test]
fn import_revision_requests_new_history_and_discards_an_in_flight_reply() {
    let sync = Revision::default();
    let before = Some(ArtifactDigest::from_bytes(b"before import"));
    let after = Some(ArtifactDigest::from_bytes(b"after import"));
    let mut state = State::from_cache(Some(History::default()), sync.clone());
    state.observe_import_revision(before);
    let old = state.begin_read(sync.clone());
    state.observe_import_revision(after);
    assert!(state.needs_read(&sync));
    let current = state.begin_read(sync.clone());
    state.complete_read(&old, Err("stale response".into()));
    assert!(matches!(state.read, ReadState::Loading));
    state.complete_read(&current, Ok(Some(History::default())));
    assert!(!state.needs_read(&sync));
}
