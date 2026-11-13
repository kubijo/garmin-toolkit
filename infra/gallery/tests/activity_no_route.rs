use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use garmin_i18n::{Language, Translations};
use garmin_model::{
    activity::{
        ActivityDuration, ActivityMetrics, ActivitySport, ActivitySummary, ActivityTotals,
        TimeRange,
    },
    identity::UnitSystem,
    value::Timestamp,
};
use garmin_service_api::{ActivityRecordingSnapshot, ActivitySampleSnapshot};
use garmin_ui::activity;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct NoTiles(Arc<AtomicUsize>);
impl activity::map_runtime::Backend for NoTiles {
    fn fetch(
        &self,
        _: activity::map_runtime::TileCoordinates,
        reply: activity::map_runtime::TileReply,
    ) {
        self.0.fetch_add(1, Ordering::Relaxed);
        reply(Err("route-free fixture must not fetch tiles".into()));
    }
}

fn harness() -> (Harness<'static, ()>, Arc<AtomicUsize>) {
    let intl = Translations::bundled()
        .expect("bundled translations")
        .formatter(Language::English)
        .expect("English formatter");
    let start = Timestamp::from_unix_seconds(0).expect("fixture start");
    let end = Timestamp::from_unix_seconds(60).expect("fixture end");
    let duration = ActivityDuration::from_milliseconds(60_000);
    let summary = ActivitySummary::from_parts(
        ActivitySport::Cycling,
        TimeRange::from_parts(start, end).expect("fixture time range"),
        ActivityTotals::from_parts(duration, duration, None, None, None, None)
            .expect("fixture totals"),
        ActivityMetrics::default(),
    );
    let presentation = activity::Presentation::from_summary(
        summary,
        "Route-free fixture",
        &intl,
        UnitSystem::Metric,
    );
    let recording = ActivityRecordingSnapshot {
        laps: Vec::new(),
        timer_events: Vec::new(),
        samples: (0_i32..=60)
            .map(|seconds| ActivitySampleSnapshot {
                timestamp: Timestamp::from_unix_seconds(i64::from(seconds))
                    .expect("fixture sample"),
                coordinate: None,
                elevation_meters: Some(40.0 + f64::from(seconds % 10)),
                distance: None,
                speed: None,
                heart_rate: None,
                cadence: None,
                power: None,
                temperature_millicelsius: None,
            })
            .collect(),
    };
    let requests = Arc::new(AtomicUsize::new(0));
    let runtime = activity::map_runtime::MapRuntimeHandle::new(
        NoTiles(Arc::clone(&requests)),
        activity::map_runtime::Renderer::software(),
    )
    .on_ui_thread();
    let mut viewer = activity::Workspace::new(&runtime);
    let presentations = [presentation];
    let mut installed = false;
    let view = Harness::builder().with_size([800.0, 800.0]).build_ui_state(
        move |ui, (): &mut ()| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            let _ = viewer.show(
                ui,
                &intl,
                &activity::WorkspaceProps {
                    today: jiff::civil::date(1970, 1, 1),
                    items: &[presentations[0].item_props()],
                    presentations: &presentations,
                    selected: Some(0),
                    recording: Some(&recording),
                    recording_key: Some("no-gps"),
                    units: UnitSystem::Metric,
                    empty_list: "Empty",
                    empty_detail: "Select",
                    no_route: "No recorded route",
                },
            );
        },
        (),
    );
    (view, requests)
}

#[test]
fn route_free_recordings_show_charts_without_a_map_or_zoom_controls() {
    let (mut view, requests) = harness();
    view.run();
    let target =
        |name: &'static str| By::new().predicate(move |node| node.author_id() == Some(name));
    assert!(view.query(target("map.zoom-in")).is_none());
    assert!(view.query(target("map.fit")).is_none());
    assert!(view.query(target("playback.toggle")).is_some());
    assert!(
        view.get(target("playback.toggle"))
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        view.get(target("playback.speed"))
            .accesskit_node()
            .is_disabled()
    );
    assert!(view.query(target("activity.no-route-icon")).is_some());
    let placeholder = view.get(target("activity.no-route-placeholder")).rect();
    let controls = view.get(target("activity.no-route-controls")).rect();
    assert!((placeholder.height() - 96.0).abs() < 1.0);
    assert!((controls.center().x - placeholder.center().x).abs() < 1.0);
    assert!((placeholder.bottom() - controls.bottom() - 16.0).abs() < 1.0);
    let notice = view.get(target("activity.no-route"));
    let chart = view.get(target("chart.0"));
    assert!(chart.rect().top() - notice.rect().bottom() < 160.0);
    assert_eq!(requests.load(Ordering::Relaxed), 0);
}

#[test]
#[ignore = "renders a route-free recording with a headless GPU"]
fn capture_route_free_recording() -> Result<(), Box<dyn std::error::Error>> {
    let (mut view, _) = harness();
    view.run();
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/cloud-browser");
    std::fs::create_dir_all(&out)?;
    view.render()?.save(out.join("no-route.png"))?;
    Ok(())
}
