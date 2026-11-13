//! Profile controls preserve independent preferences and scope draft colors by identity.
use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use gallery::egui;
use garmin_color::{Color, swatch};
use garmin_model::identity::ProfilePreferences;
use garmin_ui::profile_settings::{self, Action};

enum RequestState {
    Idle,
    Loading,
}

struct Model {
    cloud: bool,
    profile: u8,
    accent: Option<Color>,
    preferences: ProfilePreferences,
    disabled: bool,
    key: Option<garmin_model::garmin_connect::PersonalApiKey>,
    facts: Option<garmin_model::garmin_connect::AccountFacts>,
    checks: u32,
    reject: bool,
    activities: Option<garmin_model::garmin_connect::ActivityResponse>,
    activity_request: RequestState,
    load_progress: Option<garmin_model::garmin_connect::LoadProgress>,
    requests: Vec<garmin_model::garmin_connect::ActivityRequest>,
    batch: Option<profile_settings::connect_activities::Batch>,
}

fn harness() -> Harness<'static, Model> {
    harness_for(garmin_i18n::Language::English, [760.0, 1000.0])
}

fn test_facts() -> garmin_model::garmin_connect::AccountFacts {
    garmin_model::garmin_connect::AccountFacts {
        display_name: Some("Garmin Rider".into()),
        locale: Some("cs".into()),
        measurement_system: Some("metric".into()),
        time_zone: Some("Europe/Prague".into()),
    }
}

fn harness_for(language: garmin_i18n::Language, size: [f32; 2]) -> Harness<'static, Model> {
    let intl = garmin_i18n::Translations::bundled()
        .expect("bundled translations")
        .formatter(language)
        .expect("locale formatter");
    let mut installed = false;
    Harness::builder()
        .with_size(size)
        .with_max_steps(32)
        .build_ui_state(
            move |ui, state: &mut Model| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                let show: fn(&mut egui::Ui, &profile_settings::Props<'_>) -> Option<Action> =
                    if state.cloud {
                        profile_settings::connect_activities::show
                    } else {
                        profile_settings::show
                    };
                if let Some(action) = show(
                    ui,
                    &profile_settings::Props {
                        id: egui::Id::new(state.profile),
                        intl: &intl,
                        preferences: state.preferences,
                        accent: state.accent,
                        profile: garmin_ui::profile::ProfileProps {
                            display_name: "Same name",
                            accent: state.accent.unwrap_or(swatch::ACTION),
                            avatar: None,
                        },
                        picture_enabled: false,
                        garmin_connect: profile_settings::GarminConnectProps {
                            now: "2026-11-10T12:00:00Z".parse().expect("fixture timestamp"),
                            today: jiff::civil::date(2026, 11, 13),
                            configured: state.key.is_some(),
                            checking: false,
                            facts: state.facts.as_ref(),
                            activities: state.activities.as_ref(),
                            activities_busy: matches!(
                                state.activity_request,
                                RequestState::Loading
                            ),
                            load_progress: state.load_progress.as_ref(),
                            batch: state.batch.as_ref(),
                        },
                        disabled: state.disabled,
                    },
                ) {
                    match action {
                        Action::StartGarminConnectPageImport => start_page_import(state),
                        Action::CancelGarminConnectPageImport => {
                            if let Some(batch) = &mut state.batch {
                                batch.cancel();
                            }
                        }
                        Action::UpdateAccent(accent) => state.accent = accent,
                        Action::UpdatePreferences(preferences) => state.preferences = preferences,
                        Action::ChoosePicture => {}
                        Action::GarminConnectActivity(request) => state.requests.push(request),
                        Action::UpdateGarminConnectKey(key) => {
                            if state.reject {
                                state.facts = None;
                            } else {
                                state.facts = key.as_ref().map(|_| test_facts());
                                state.key = key;
                            }
                        }
                        Action::TestGarminConnectKey => {
                            state.checks += 1;
                            state.facts = Some(test_facts());
                        }
                    }
                }
            },
            Model {
                cloud: false,
                profile: 0,
                accent: None,
                disabled: false,
                key: None,
                facts: None,
                checks: 0,
                reject: false,
                activities: None,
                activity_request: RequestState::Idle,
                load_progress: None,
                requests: Vec::new(),
                batch: None,
                preferences: ProfilePreferences::default()
                    .with_show_hidden_files(true)
                    .with_inline_file_windows(true),
            },
        )
}

fn start_page_import(state: &mut Model) {
    if let Some(garmin_model::garmin_connect::ActivityResponse::Page { activities, .. }) =
        &state.activities
    {
        let mut batch =
            profile_settings::connect_activities::Batch::new(activities).expect("one page");
        if let Some(request) = batch.next_request() {
            state.requests.push(request);
        }
        state.batch = Some(batch);
    }
}

#[test]
fn scrolling_from_the_empty_side_moves_the_heading_and_reaches_preferences() {
    let mut view = harness_for(garmin_i18n::Language::English, [960.0, 480.0]);
    view.run();
    assert!(view.get_by_label("Profile settings").rect().top() > 0.0);
    view.hover_at(egui::pos2(900.0, 240.0));
    view.event(egui::Event::MouseWheel {
        phase: egui::TouchPhase::Move,
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -600.0),
        modifiers: egui::Modifiers::NONE,
    });
    view.run();
    assert!(
        view.query_by_label("Profile settings")
            .is_none_or(|node| node.rect().bottom() <= 0.0)
    );
    let field = view
        .get(By::new().predicate(|node| node.author_id() == Some("profile.file-windows.inline")));
    assert!(field.rect().top() >= 0.0 && field.rect().bottom() <= 480.0);
}

#[test]
fn personal_api_key_can_be_saved_replaced_and_removed() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.run();
    let field =
        || By::new().predicate(|node| node.author_id() == Some("profile.garmin-connect.key"));
    for key in ["first-test-key", "replacement-test-key"] {
        if view.state().key.is_some() {
            assert!(view.query(field()).is_none());
            view.get_by_label("Replace key").click();
            view.run();
        }
        view.get(field()).click();
        view.get(field()).type_text(key);
        view.run();
        view.get_by_label("Verify and save").click();
        view.run();
        assert_eq!(
            view.state()
                .key
                .as_ref()
                .expect("key saved by settings action")
                .expose(),
            key
        );
        assert!(view.query_by_label("Personal API key saved").is_some());
        assert!(view.query(field()).is_none());
    }
    view.get_by_label("Remove key").click();
    view.run();
    assert!(view.state().key.is_none());
    assert!(view.query_by_label("No Personal API key saved").is_some());
}

#[test]
fn unsaved_key_drafts_do_not_cross_profile_identity() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.run();
    let field =
        || By::new().predicate(|node| node.author_id() == Some("profile.garmin-connect.key"));
    view.get(field()).click();
    view.get(field()).type_text("unsaved-test-key");
    view.run();
    view.state_mut().profile = 1;
    view.run();
    assert!(
        view.get_by_label("Verify and save")
            .accesskit_node()
            .is_disabled()
    );
    assert!(view.state().key.is_none());
}

#[test]
fn browsing_cloud_activities_only_requests_a_preview_until_explicit_import() {
    use garmin_model::garmin_connect::{ActivityRequest, ActivityResponse, RemoteActivity};
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-key".into())
            .expect("fixture key"),
    );
    view.run();
    assert_eq!(view.state().requests.len(), 0);
    view.state_mut().cloud = true;
    view.run();
    assert_eq!(
        view.state().requests,
        [ActivityRequest::Load {
            start: 0,
            filters: garmin_model::garmin_connect::ActivityFilters::default()
        }]
    );
    view.state_mut().activities = Some(ActivityResponse::Page {
        start: 0,
        filters: garmin_model::garmin_connect::ActivityFilters::default(),
        total: 1,
        cache: garmin_model::garmin_connect::CacheStatus::default(),
        warning: None,
        has_more: false,
        activities: vec![RemoteActivity {
            id: 42,
            imported: false,
            sport: Some(garmin_model::activity::ActivitySport::Cycling),
            name: "Road ride".into(),
            started_at: Some("2026-11-10 10:00:00".into()),
            distance_metres: Some(13230.0),
            duration_seconds: Some(3600.0),
        }],
    });
    view.run();
    assert!(view.query_by_label("Import FIT").is_none());
    let row_bounds = view.get_by_label("Road ride").rect();
    assert!(
        !view
            .get_by_label("Road ride")
            .accesskit_node()
            .is_disabled()
    );
    view.get_by_label("Road ride").click();
    view.run();
    assert_eq!(
        view.state().requests,
        [
            ActivityRequest::Load {
                start: 0,
                filters: garmin_model::garmin_connect::ActivityFilters::default()
            },
            ActivityRequest::Preview { id: 42 }
        ],
        "row bounds: {row_bounds:?}"
    );
}

fn cloud_page(start: u32) -> garmin_model::garmin_connect::ActivityResponse {
    use garmin_model::{
        activity::ActivitySport,
        garmin_connect::{ActivityResponse, RemoteActivity},
    };
    ActivityResponse::Page {
        start,
        filters: garmin_model::garmin_connect::ActivityFilters::default(),
        total: 80,
        cache: garmin_model::garmin_connect::CacheStatus {
            total: 80,
            downloaded: 80,
            unavailable: 0,
        },
        warning: None,
        has_more: true,
        activities: (0..20)
            .map(|index| RemoteActivity {
                id: i64::from(index + 1),
                imported: index == 2,
                sport: Some(if index % 2 == 0 {
                    ActivitySport::Cycling
                } else {
                    ActivitySport::Running
                }),
                name: format!(
                    "{} {}",
                    if index % 2 == 0 {
                        "Hamina Road Cycling"
                    } else {
                        "Evening run"
                    },
                    index + 1
                ),
                started_at: Some("2026-11-10 07:12:38".into()),
                distance_metres: Some(13230.0),
                duration_seconds: Some(4320.0),
            })
            .collect(),
    }
}

#[test]
fn cloud_browser_preserves_the_current_page_and_keeps_paging_controls_visible() {
    use garmin_model::garmin_connect::ActivityRequest;
    let mut view = harness_for(garmin_i18n::Language::English, [1280.0, 900.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-key".into())
            .expect("fixture key"),
    );
    view.state_mut().activities = Some(cloud_page(20));
    view.run();
    assert!(view.query_by_label("Hamina Road Cycling 1").is_none());
    view.state_mut().cloud = true;
    view.run();
    assert_eq!(view.state().requests.len(), 0);
    assert!(view.query_by_label("Activities 21–40 of 80").is_some());
    let next = view.get_by_label("Next page");
    assert!(next.rect().bottom() < 900.0);
    next.click();
    view.run();
    assert_eq!(
        view.state().requests,
        [ActivityRequest::List {
            start: 40,
            filters: garmin_model::garmin_connect::ActivityFilters::default()
        }]
    );
    view.state_mut().cloud = false;
    view.run();
    assert!(view.query_by_label("Hamina Road Cycling 1").is_none());
}

#[test]
fn cloud_filters_validate_dates_reset_paging_and_preserve_applied_queries() {
    use garmin_model::garmin_connect::{ActivityFilters, ActivityRequest, ActivityResponse};
    let mut view = harness_for(garmin_i18n::Language::English, [1280.0, 900.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("fixture-key".into())
            .expect("fixture key"),
    );
    view.state_mut().activities = Some(cloud_page(20));
    view.state_mut().cloud = true;
    view.run();
    let from = || By::new().predicate(|node| node.author_id() == Some("cloud.filters.from"));
    view.get(from()).click();
    view.get(from()).type_text("2026-02-30");
    view.run();
    assert!(
        view.get_by_label("Apply filters")
            .accesskit_node()
            .is_disabled()
    );
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(from()).type_text("2026-11-01");
    view.run();
    view.get_by_label("Apply filters").click();
    view.run();
    let filters = ActivityFilters {
        from: Some("2026-11-01".parse().expect("fixture date")),
        ..ActivityFilters::default()
    };
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::List {
            start: 0,
            filters: filters.clone()
        })
    );
    let mut page = cloud_page(0);
    if let ActivityResponse::Page {
        filters: applied, ..
    } = &mut page
    {
        *applied = filters.clone();
    }
    view.state_mut().activities = Some(page);
    view.run();
    view.get_by_label("Refresh").click();
    view.run();
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::Load {
            start: 0,
            filters: filters.clone()
        })
    );
    view.get_by_label("Next page").click();
    view.run();
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::List { start: 20, filters })
    );
    view.get_by_label("Clear filters").click();
    view.run();
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::List {
            start: 0,
            filters: ActivityFilters::default()
        })
    );
}

#[test]
#[ignore = "renders activity icons and date filter popups"]
fn capture_filter_controls() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/cloud-filters");
    std::fs::create_dir_all(&out)?;
    for (name, size, theme) in [
        ("dark", [840.0, 900.0], egui::ThemePreference::Dark),
        ("light", [840.0, 900.0], egui::ThemePreference::Light),
        ("narrow", [400.0, 900.0], egui::ThemePreference::Dark),
    ] {
        let mut view = harness_for(garmin_i18n::Language::English, size);
        view.state_mut().key = Some(
            garmin_model::garmin_connect::PersonalApiKey::from_string("fixture-key".into())
                .expect("fixture key"),
        );
        view.state_mut().activities = Some(cloud_page(0));
        view.state_mut().cloud = true;
        view.ctx.set_theme(theme);
        view.run();
        view.get(By::new().predicate(|node| node.author_id() == Some("cloud.filters.type")))
            .click();
        view.run();
        view.render()?.save(out.join(format!("{name}-types.png")))?;
        view.get_by_label("Running").click();
        view.run();
        view.get_by_label("Choose date for From date").click();
        view.run();
        view.render()?
            .save(out.join(format!("{name}-calendar.png")))?;
    }
    Ok(())
}

#[test]
fn empty_account_filter_disables_navigation_and_bulk_import() {
    use garmin_model::garmin_connect::{ActivityFilters, ActivityResponse, CacheStatus};
    let mut view = harness_for(garmin_i18n::Language::English, [1280.0, 900.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("fixture-key".into())
            .expect("fixture key"),
    );
    view.state_mut().activities = Some(ActivityResponse::Page {
        start: 0,
        filters: ActivityFilters {
            not_imported: true,
            ..ActivityFilters::default()
        },
        total: 0,
        cache: CacheStatus {
            total: 80,
            downloaded: 80,
            unavailable: 0,
        },
        warning: None,
        activities: Vec::new(),
        has_more: false,
    });
    view.state_mut().cloud = true;
    view.run();
    assert!(
        view.get_by_label("Import all on this page")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        view.get_by_label("Next page")
            .accesskit_node()
            .is_disabled()
    );
    assert!(view.query_by_label("Activities 0–0 of 0").is_some());
}

#[test]
fn narrow_filter_prose_keeps_complete_sentences_on_separate_lines() {
    let mut view = harness_for(garmin_i18n::Language::English, [400.0, 760.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("fixture-key".into())
            .expect("fixture key"),
    );
    view.state_mut().activities = Some(cloud_page(0));
    view.state_mut().cloud = true;
    view.run();
    assert!(
        view.query_by_label(
            "Dates use the activity’s local time.\nFilters search the whole cached account."
        )
        .is_some()
    );
}

#[test]
fn fuzzy_search_is_a_single_account_query_and_imports_refill_the_cached_page() {
    use garmin_model::garmin_connect::{ActivityFilters, ActivityRequest, ActivityResponse};
    let mut view = harness_for(garmin_i18n::Language::English, [1280.0, 900.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("fixture-key".into())
            .expect("fixture key"),
    );
    view.state_mut().activities = Some(cloud_page(20));
    view.state_mut().cloud = true;
    view.run();
    let search = || By::new().predicate(|node| node.author_id() == Some("cloud.filters.search"));
    view.get(search()).click();
    view.get(search()).type_text("hamnia coastl");
    view.run();
    view.get_by_label("Apply filters").click();
    view.run();
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::List {
            start: 0,
            filters: ActivityFilters {
                search: "hamnia coastl".into(),
                ..ActivityFilters::default()
            },
        })
    );
    let mut page = cloud_page(20);
    let filters = ActivityFilters {
        not_imported: true,
        ..ActivityFilters::default()
    };
    if let ActivityResponse::Page {
        filters: applied,
        activities,
        ..
    } = &mut page
    {
        *applied = filters.clone();
        for activity in activities {
            activity.imported = false;
        }
    }
    page.merge(ActivityResponse::Imported {
        id: 3,
        activities: 1,
        duplicate: false,
    });
    assert!(!profile_settings::connect_activities::clears_batch(
        &ActivityRequest::List {
            start: 20,
            filters: filters.clone()
        },
        Some(&page)
    ));
    view.state_mut().activities = Some(page);
    view.run();
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::List { start: 20, filters })
    );
}

#[test]
fn page_import_disables_paging_but_keeps_cancellation_available() {
    use garmin_model::garmin_connect::ActivityRequest;
    let mut view = harness_for(garmin_i18n::Language::English, [1280.0, 900.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-key".into())
            .expect("fixture key"),
    );
    view.state_mut().cloud = true;
    view.state_mut().activities = Some(cloud_page(0));
    view.run();
    view.get_by_label("Import all on this page").click();
    view.run_steps(3);
    assert_eq!(view.state().requests, [ActivityRequest::Prepare { id: 1 }]);
    let batch = view.state().batch.as_ref().expect("started batch");
    assert_eq!((batch.counts.total, batch.counts.skipped), (20, 1));
    assert!(
        view.get_by_label("Next page")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        view.get_by_label("Import all on this page")
            .accesskit_node()
            .is_disabled()
    );
    view.get_by_label("Cancel import").click();
    view.run_steps(3);
    assert!(view.state().batch.as_ref().expect("batch").cancelled());
    assert!(
        view.get_by_label("Stopping…")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(view.state().requests.len(), 1);
    let step = view
        .state_mut()
        .batch
        .as_mut()
        .expect("batch")
        .complete(Err(garmin_model::garmin_connect::ActivityFailure {
            message: "Connection lost".into(),
            kind: garmin_model::garmin_connect::ActivityFailureKind::Session,
        }));
    assert!(step.request.is_none());
    view.run();
    assert!(view.query_by_label("Page import cancelled").is_some());
    assert!(
        !view
            .get_by_label("Next page")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn completed_batch_does_not_hide_later_loading_feedback() {
    let mut view = harness_for(garmin_i18n::Language::English, [1280.0, 900.0]);
    view.state_mut().cloud = true;
    view.state_mut().activities = Some(cloud_page(0));
    view.state_mut().batch = Some(
        profile_settings::connect_activities::Batch::new(&[])
            .expect("empty page forms a completed batch"),
    );
    view.state_mut().activity_request = RequestState::Loading;
    view.run_steps(4);
    assert!(view.query_by_label("Loading activity data…").is_some());
}

#[test]
fn account_load_reports_real_phases_and_file_counts_without_a_cached_page() {
    use garmin_model::garmin_connect::LoadProgress;
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 800.0]);
    view.state_mut().cloud = true;
    view.state_mut().activity_request = RequestState::Loading;
    view.state_mut().load_progress = Some(LoadProgress::Indexing {
        discovered: 120,
        pages: 6,
    });
    view.run_steps(4);
    assert!(
        view.query_by_label("Reading activity summaries · 120 found")
            .is_some()
    );
    assert!(view.query_by_label("Pages read: 6").is_some());
    view.state_mut().load_progress = Some(LoadProgress::Downloading {
        total: 200,
        downloaded: 40,
        reused: 80,
        unavailable: 2,
        current: Some("Hamina Road Cycling".into()),
    });
    view.run_steps(4);
    assert!(
        view.query_by_label("Downloading Hamina Road Cycling")
            .is_some()
    );
    assert!(
        view.query_by_label(
            "Files 122/200 · Downloaded 40 · Already cached 80 · Unavailable 2 · Remaining 78"
        )
        .is_some()
    );
    assert_eq!(view.state().requests.len(), 0);
}

#[test]
#[ignore = "renders live account-load progress with a headless GPU"]
fn capture_account_load_progress() -> Result<(), Box<dyn std::error::Error>> {
    use garmin_model::garmin_connect::LoadProgress;
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/cloud-progress");
    std::fs::create_dir_all(&out)?;
    for (name, size, theme) in [
        ("dark", [840.0, 600.0], egui::ThemePreference::Dark),
        ("light", [840.0, 600.0], egui::ThemePreference::Light),
        ("narrow", [400.0, 600.0], egui::ThemePreference::Dark),
    ] {
        let mut view = harness_for(garmin_i18n::Language::English, size);
        view.state_mut().cloud = true;
        view.state_mut().activity_request = RequestState::Loading;
        view.ctx.set_theme(theme);
        view.state_mut().load_progress = Some(LoadProgress::Indexing {
            discovered: 120,
            pages: 6,
        });
        view.run_steps(5);
        view.render()?
            .save(out.join(format!("{name}-indexing.png")))?;
        view.state_mut().load_progress = Some(LoadProgress::Downloading {
            total: 200,
            downloaded: 40,
            reused: 80,
            unavailable: 2,
            current: Some("Hamina Road Cycling".into()),
        });
        view.run_steps(5);
        view.render()?
            .save(out.join(format!("{name}-downloading.png")))?;
    }
    Ok(())
}

#[test]
fn imported_rows_remain_browsable_and_pager_can_select_a_page() {
    use garmin_model::garmin_connect::ActivityRequest;
    let mut view = harness_for(garmin_i18n::Language::English, [960.0, 760.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-key".into())
            .expect("fixture key"),
    );
    view.state_mut().cloud = true;
    view.state_mut().activities = Some(cloud_page(20));
    view.run();
    let imported = view.get_by_label("Hamina Road Cycling 3");
    assert!(
        imported
            .accesskit_node()
            .description()
            .is_some_and(|text| text.contains("Imported"))
    );
    imported.click();
    view.run();
    assert_eq!(view.state().requests, [ActivityRequest::Preview { id: 3 }]);
    view.get(By::new().predicate(|node| node.value().as_deref() == Some("Page 2")))
        .click();
    view.run();
    view.get_by_label("Page 1").click();
    view.run();
    assert_eq!(
        view.state().requests.last(),
        Some(&ActivityRequest::List {
            start: 0,
            filters: garmin_model::garmin_connect::ActivityFilters::default()
        })
    );
}

#[test]
fn testing_a_saved_key_shows_service_facts() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-key".into())
            .expect("fixture key"),
    );
    view.run();
    view.get_by_label("Test connection").click();
    view.run();
    assert_eq!(view.state().checks, 1);
    for label in [
        "Verified with Garmin Connect",
        "Display name",
        "Garmin Rider",
        "Locale",
        "cs",
        "Units",
        "metric",
        "Time zone",
    ] {
        assert!(view.query_by_label(label).is_some(), "missing {label}");
    }
}

#[test]
fn uuid_display_names_are_presented_as_profile_identifiers() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.state_mut().facts = Some(garmin_model::garmin_connect::AccountFacts {
        display_name: Some("df38d475-c521-457c-810f-71c5aa657fe9".into()),
        ..test_facts()
    });
    view.run();
    assert!(view.query_by_label("Garmin profile ID").is_some());
    assert!(view.query_by_label("Display name").is_none());
}

#[test]
fn failed_verification_keeps_the_new_key_in_the_editor_for_retry() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.state_mut().reject = true;
    view.run();
    let field = By::new().predicate(|node| node.author_id() == Some("profile.garmin-connect.key"));
    view.get(field).click();
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.garmin-connect.key")))
        .type_text("retry-key");
    view.run();
    view.get_by_label("Verify and save").click();
    view.run();
    assert!(view.state().key.is_none());
    assert!(
        !view
            .get_by_label("Verify and save")
            .accesskit_node()
            .is_disabled()
    );
    view.state_mut().reject = false;
    view.get_by_label("Verify and save").click();
    view.run();
    assert_eq!(
        view.state()
            .key
            .as_ref()
            .expect("key saved after retry")
            .expose(),
        "retry-key"
    );
}

#[test]
fn key_entry_and_removal_follow_settings_busy_state() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-test-key".into())
            .expect("valid fixture key"),
    );
    view.state_mut().disabled = true;
    view.run();
    let field = By::new().predicate(|node| node.author_id() == Some("profile.garmin-connect.key"));
    assert!(view.query(field).is_none());
    assert!(
        view.get_by_label("Remove key")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        view.get_by_label("Replace key")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn cancelling_key_replacement_preserves_the_saved_connection() {
    let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1600.0]);
    view.state_mut().key = Some(
        garmin_model::garmin_connect::PersonalApiKey::from_string("saved-key".into())
            .expect("fixture key"),
    );
    view.run();
    view.get_by_label("Replace key").click();
    view.run();
    assert!(view.query_by_label("New Personal API key").is_some());
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.garmin-connect.key")))
        .type_text("discarded-key");
    view.run();
    view.get_by_label("Cancel").click();
    view.run();
    assert!(view.query_by_label("New Personal API key").is_none());
    assert_eq!(
        view.state().key.as_ref().expect("saved key").expose(),
        "saved-key"
    );
    view.get_by_label("Test connection").click();
    view.run();
    assert_eq!(view.state().checks, 1);
}

#[test]
fn narrow_czech_settings_keep_theme_choices_visible_and_clickable() {
    let mut view = harness_for(garmin_i18n::Language::Czech, [320.0, 1000.0]);
    view.run();
    for target in [
        "profile.units.metric",
        "profile.units.imperial",
        "profile.language.english",
        "profile.language.czech",
        "profile.file-windows.separate",
        "profile.file-windows.inline",
    ] {
        let bounds = view
            .get(By::new().predicate(|node| node.author_id() == Some(target)))
            .rect();
        assert!(
            bounds.left() >= 0.0 && bounds.right() <= 320.0,
            "{target}: {bounds:?}"
        );
    }
    for label in ["Automaticky", "Tmavý", "Světlý"] {
        let bounds = view.get_by_label(label).rect();
        assert!(
            bounds.left() >= 0.0 && bounds.right() <= 320.0,
            "{label}: {bounds:?}"
        );
    }
    assert!(
        view.get_by_label("Světlý").rect().top()
            >= view.get_by_label("Automaticky").rect().bottom(),
        "wrap whole buttons instead of splitting their labels"
    );
    view.get_by_label("Světlý").click();
    view.run();
    assert_eq!(
        view.state().preferences.theme(),
        garmin_model::identity::ThemePreference::Light
    );
    assert!(view.state().preferences.show_hidden_files());
    assert!(view.state().preferences.inline_file_windows());
}

#[test]
fn custom_accent_requires_valid_input_and_apply_and_can_reset() {
    let mut view = harness();
    view.run();
    let default_ring = avatar_ring(&view);
    open_picker(&mut view);
    let hex = || By::new().role(egui::accesskit::Role::TextInput);
    let apply = || By::new().predicate(|node| node.author_id() == Some("profile.accent.apply"));
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#FF");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    assert!(view.get(apply()).accesskit_node().is_disabled());
    assert_eq!(view.state().accent, None);
    open_picker(&mut view);
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#C855A8");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    assert_eq!(view.state().accent, None);
    assert_eq!(avatar_ring(&view), egui::Color32::from_rgb(200, 85, 168));
    let apply_center = view.get(apply()).rect().center();
    view.hover_at(apply_center);
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    view.get(apply()).click();
    view.run();
    assert_eq!(view.state().accent, Some(Color::from_rgb(200, 85, 168)));
    assert_eq!(avatar_ring(&view), egui::Color32::from_rgb(200, 85, 168));
    assert!(view.state().preferences.inline_file_windows());
    assert!(view.state().preferences.show_hidden_files());
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.reset")))
        .click();
    view.run();
    assert_eq!(view.state().accent, None);
    assert_eq!(avatar_ring(&view), default_ring);
}

fn avatar_ring(view: &Harness<'_, Model>) -> egui::Color32 {
    view.output()
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::epaint::Shape::Circle(circle) = &shape.shape
                && (circle.radius - 32.0).abs() < f32::EPSILON
            {
                Some(circle.fill)
            } else {
                None
            }
        })
        .expect("profile avatar's outer circle")
}

#[test]
fn window_and_theme_changes_preserve_other_profile_preferences() {
    let mut view = harness();
    view.run();
    view.get_by_label("Separate window").click();
    view.run();
    assert!(!view.state().preferences.inline_file_windows());
    view.get_by_label("Light").click();
    view.run();
    assert!(view.state().preferences.show_hidden_files());
    assert!(!view.state().preferences.inline_file_windows());
}

#[test]
fn binary_preferences_are_visible_radios_and_preserve_other_settings() {
    let mut view = harness();
    view.run();
    for label in [
        "Metric",
        "Imperial",
        "English",
        "Čeština",
        "Separate window",
        "Inside the app",
    ] {
        assert_eq!(
            view.get_by_label(label).accesskit_node().role(),
            egui::accesskit::Role::RadioButton
        );
    }
    view.get_by_label("Imperial").click();
    view.run();
    assert_eq!(
        view.state().preferences.unit_system(),
        garmin_model::identity::UnitSystem::Imperial
    );
    view.get_by_label("Čeština").click();
    view.run();
    assert_eq!(
        view.state().preferences.language(),
        garmin_model::identity::LanguagePreference::Czech
    );
    assert_eq!(
        view.state().preferences.unit_system(),
        garmin_model::identity::UnitSystem::Imperial
    );
    assert!(view.state().preferences.show_hidden_files());
    assert!(view.state().preferences.inline_file_windows());
    view.get_by_label("Separate window").focus();
    view.key_press(egui::Key::Space);
    view.run();
    assert!(!view.state().preferences.inline_file_windows());
}

#[test]
fn radio_controls_have_correct_cursors_and_cannot_change_when_disabled() {
    let mut view = harness();
    view.run();
    let center = view.get_by_label("Imperial").rect().center();
    view.hover_at(center);
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    view.state_mut().disabled = true;
    view.run();
    assert!(view.get_by_label("Imperial").accesskit_node().is_disabled());
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::NotAllowed
    );
    let initial = view.state().preferences;
    view.get_by_label("Imperial").click();
    view.run();
    assert_eq!(view.state().preferences, initial);
}

#[test]
fn color_drafts_do_not_cross_profiles_with_the_same_name() {
    let mut view = harness();
    view.run();
    let default_ring = avatar_ring(&view);
    open_picker(&mut view);
    let hex = || By::new().role(egui::accesskit::Role::TextInput);
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#C855A8");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    view.state_mut().profile = 1;
    view.run();
    assert_eq!(avatar_ring(&view), default_ring);
    assert!(
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.apply")))
            .accesskit_node()
            .is_disabled()
    );
    open_picker(&mut view);
    assert_ne!(view.get(hex()).value().as_deref(), Some("#C855A8"));
}

fn open_picker(view: &mut Harness<'_, Model>) {
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.picker")))
        .click();
    view.run();
}

#[test]
fn accent_controls_show_pointer_when_enabled_and_not_allowed_when_disabled() {
    let mut view = harness();
    view.run();
    for (target, expected) in [
        ("profile.accent.picker", egui::CursorIcon::PointingHand),
        ("profile.accent.apply", egui::CursorIcon::NotAllowed),
        ("profile.accent.reset", egui::CursorIcon::NotAllowed),
    ] {
        let center = view
            .get(By::new().predicate(|node| node.author_id() == Some(target)))
            .rect()
            .center();
        view.hover_at(center);
        view.run();
        assert_eq!(
            view.output().platform_output.cursor_icon,
            expected,
            "{target}"
        );
    }
    view.state_mut().disabled = true;
    view.run();
    let center = view
        .get(By::new().predicate(|node| node.author_id() == Some("profile.accent.picker")))
        .rect()
        .center();
    view.hover_at(center);
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::NotAllowed
    );
}

#[test]
fn picker_keeps_alpha_when_applying() {
    let mut view = harness();
    view.run();
    let style = view.ctx.global_style();
    open_picker(&mut view);
    assert_eq!(view.ctx.global_style(), style);
    let hex = || By::new().role(egui::accesskit::Role::TextInput);
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#00FF0080");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.apply")))
        .click();
    view.run();
    assert_eq!(view.state().accent, Some(Color::from_rgba(0, 255, 0, 128)));
}

#[test]
#[ignore = "renders the open accent picker with a headless GPU"]
fn capture_accent_picker() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/profile-accent");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("dark", egui::ThemePreference::Dark),
        ("light", egui::ThemePreference::Light),
    ] {
        let mut view = harness();
        view.run();
        view.ctx.set_theme(theme);
        view.run();
        open_picker(&mut view);
        view.render()?.save(out.join(format!("{name}.png")))?;
        let hex = || By::new().role(egui::accesskit::Role::TextInput);
        view.get(hex()).click();
        view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        view.get(hex()).type_text("#C855A8");
        view.key_press(egui::Key::Enter);
        view.key_press(egui::Key::Escape);
        view.run();
        view.render()?
            .save(out.join(format!("{name}-preview.png")))?;
    }
    Ok(())
}

#[test]
#[ignore = "renders the Garmin Connect settings with a headless GPU"]
fn capture_connect_settings() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/gallery/connect-settings");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("dark", egui::ThemePreference::Dark),
        ("light", egui::ThemePreference::Light),
    ] {
        let mut view = harness_for(garmin_i18n::Language::English, [760.0, 1200.0]);
        view.state_mut().key = Some(
            garmin_model::garmin_connect::PersonalApiKey::from_string("preview-key".into())
                .expect("preview key"),
        );
        view.state_mut().facts = Some(test_facts());
        view.ctx.set_theme(theme);
        view.run();
        view.render()?.save(out.join(format!("{name}.png")))?;
    }
    Ok(())
}

#[test]
#[ignore = "renders the cloud activity browser with a headless GPU"]
fn capture_cloud_browser() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/cloud-browser");
    std::fs::create_dir_all(&out)?;
    for (name, size, theme) in [
        ("wide-dark", [1280.0, 900.0], egui::ThemePreference::Dark),
        ("wide-light", [1280.0, 900.0], egui::ThemePreference::Light),
        ("medium-dark", [840.0, 900.0], egui::ThemePreference::Dark),
        ("medium-light", [840.0, 900.0], egui::ThemePreference::Light),
        ("narrow", [400.0, 760.0], egui::ThemePreference::Dark),
    ] {
        let mut view = harness_for(garmin_i18n::Language::English, size);
        view.state_mut().key = Some(
            garmin_model::garmin_connect::PersonalApiKey::from_string("preview-key".into())
                .expect("fixture key"),
        );
        view.state_mut().activities = Some(cloud_page(20));
        view.state_mut().cloud = true;
        view.ctx.set_theme(theme);
        view.run();
        view.render()?.save(out.join(format!("{name}.png")))?;
        let mut batch = if let Some(garmin_model::garmin_connect::ActivityResponse::Page {
            activities,
            ..
        }) = &view.state().activities
        {
            profile_settings::connect_activities::Batch::new(activities).expect("one page")
        } else {
            unreachable!("fixture page")
        };
        assert!(batch.next_request().is_some());
        view.state_mut().batch = Some(batch);
        view.run_steps(3);
        assert!(view.get_by_label("Next page").rect().bottom() <= size[1]);
        assert!(view.get_by_label("Cancel import").rect().bottom() <= size[1]);
        view.render()?
            .save(out.join(format!("{name}-importing.png")))?;
        let batch = view.state_mut().batch.as_mut().expect("batch");
        batch.cancel();
        let failure = garmin_model::garmin_connect::ActivityFailure {
            message: "Connection lost".into(),
            kind: garmin_model::garmin_connect::ActivityFailureKind::Session,
        };
        assert!(batch.complete(Err(failure)).request.is_none());
        view.run();
        view.render()?
            .save(out.join(format!("{name}-cancelled.png")))?;
    }
    Ok(())
}
