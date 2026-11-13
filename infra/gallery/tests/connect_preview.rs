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
    garmin_connect::ActivityDetails,
    identity::UnitSystem,
    value::Timestamp,
};
use garmin_service_api::{
    ActivityRecordingSnapshot, DeviceFitPreview, DeviceFitPreviewActivity, DeviceFitPreviewContent,
};
use garmin_ui::{
    activity::map_runtime,
    device_fit_preview::{Action, Preview},
};

struct NoTiles;
impl map_runtime::Backend for NoTiles {
    fn fetch(&self, _: map_runtime::TileCoordinates, reply: map_runtime::TileReply) {
        reply(Err("route-free preview fixture".into()));
    }
}

#[derive(Default)]
struct State {
    imported: bool,
    closed: bool,
    busy: bool,
}

fn harness(imported: bool) -> Harness<'static, State> {
    let intl = Translations::bundled()
        .expect("catalog")
        .formatter(Language::English)
        .expect("English");
    let start = Timestamp::from_jiff("2026-11-04T13:06:00Z".parse().expect("start"));
    let end = Timestamp::from_jiff("2026-11-04T13:07:00Z".parse().expect("end"));
    let duration = ActivityDuration::from_milliseconds(60_000);
    let summary = ActivitySummary::from_parts(
        ActivitySport::Cycling,
        TimeRange::from_parts(start, end).expect("range"),
        ActivityTotals::from_parts(duration, duration, None, None, None, None).expect("totals"),
        ActivityMetrics::default(),
    );
    let data = DeviceFitPreview {
        file_name: "42.fit".into(),
        content: DeviceFitPreviewContent::Activities(vec![DeviceFitPreviewActivity {
            source: "FIT".into(),
            summary,
            recording: ActivityRecordingSnapshot {
                laps: Vec::new(),
                timer_events: Vec::new(),
                samples: Vec::new(),
            },
        }]),
    };
    let runtime = map_runtime::MapRuntimeHandle::new(NoTiles, map_runtime::Renderer::software())
        .on_ui_thread();
    let mut preview = Preview::new((), data, &runtime).with_cloud_details(
        ActivityDetails {
            name: Some("Morning ride".into()),
            description: Some("Windy, felt good".into()),
        },
        imported,
    );
    let mut installed = false;
    Harness::builder()
        .with_size([960.0, 900.0])
        .with_max_steps(32)
        .build_ui_state(
            move |ui, state: &mut State| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                match preview.show(
                    ui,
                    &intl,
                    state.busy,
                    UnitSystem::Metric,
                    jiff::civil::date(2026, 11, 10),
                ) {
                    Some(Action::Import(())) => state.imported = true,
                    Some(Action::Close) => state.closed = true,
                    None => {}
                }
            },
            State::default(),
        )
}

#[test]
fn cloud_title_notes_and_import_state_are_visible_before_confirmation() {
    for imported in [false, true] {
        let mut view = harness(imported);
        view.run();
        assert!(view.query_by_label("Morning ride").is_some());
        assert!(view.query_by_label("Windy, felt good").is_some());
        assert!(
            view.query_by_label("FIT activity preview · 42.fit")
                .is_some()
        );
        assert!(!view.state().imported);
        if imported {
            assert!(
                view.get_by_label("Already imported")
                    .accesskit_node()
                    .is_disabled()
            );
            assert!(view.query_by_label("Import FIT").is_none());
        } else {
            view.get_by_label("Import FIT").click();
            view.run();
            assert!(view.state().imported);
        }
    }
}

#[test]
fn header_close_uses_cancel_and_respects_busy_state() {
    for busy in [false, true] {
        let mut view = harness(false);
        view.state_mut().busy = busy;
        view.run();
        let close =
            view.get(By::new().predicate(|node| node.author_id() == Some("modal.header-close")));
        assert_eq!(close.accesskit_node().is_disabled(), busy);
        if !busy {
            let before = close.rect();
            close.click();
            view.run();
            let after = view
                .get(By::new().predicate(|node| node.author_id() == Some("modal.header-close")))
                .rect();
            assert!(
                view.state().closed,
                "header before {before:?}, after {after:?}, pointer {:?}",
                view.ctx.input(|input| input.pointer.hover_pos())
            );
            assert!(!view.state().imported);
        }
    }
}

#[test]
#[ignore = "renders cloud metadata and disabled import with a headless GPU"]
fn capture_cloud_preview() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/cloud-preview");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("dark", gallery::egui::ThemePreference::Dark),
        ("light", gallery::egui::ThemePreference::Light),
    ] {
        let mut view = harness(true);
        view.ctx.set_theme(theme);
        view.run();
        view.render()?.save(out.join(format!("{name}.png")))?;
    }
    Ok(())
}
