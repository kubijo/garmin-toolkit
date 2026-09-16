use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::{
    activity::{
        ActivityDuration, ActivityMetrics, ActivitySport, ActivitySummary, ActivityTotals, Cadence,
        Distance, HeartRate, Power, Speed, TimeRange,
    },
    identity::UnitSystem,
    route::{Coordinate, Latitude, Longitude},
    value::Timestamp,
};
use garmin_service_api::{
    ActivityLapSnapshot, ActivityRecordingSnapshot, ActivitySampleSnapshot,
    ActivityTimerEventSnapshot, ActivityTimerStateSnapshot, DeviceBrowserTarget,
    DeviceCatalogEntryKind, DeviceFitPreview, DeviceFitPreviewActivity,
};
use garmin_ui::{activity, device_fit_preview, icons};

scene_meta! { title: "Application / Activities" }

const RECORDING_START_MILLISECONDS: i64 = 1_789_453_800_000;

thread_local! {
    static WORKSPACES: crate::SceneState<(activity::Workspace, usize), 10> = const { crate::SceneState::empty() };
    static FIT_PREVIEW: crate::SceneState<device_fit_preview::Preview> = const { crate::SceneState::empty() };
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum WorkspaceSlot {
    Default,
    Compact,
    NoGps,
    Hover,
    Pinned,
    Playback,
    Lap,
    MissingMetrics,
    Loading,
    Failure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordingKind {
    Complete,
    MissingMetrics,
    NoGps,
}

#[derive(Clone, Copy)]
struct WorkspaceScene {
    recording: Option<RecordingKind>,
    cursor: Option<activity::ActivityCursor>,
    selected_lap: Option<usize>,
    fail_tiles: bool,
}

impl WorkspaceScene {
    const COMPLETE: Self = Self {
        recording: Some(RecordingKind::Complete),
        cursor: None,
        selected_lap: None,
        fail_tiles: false,
    };
}

const ITEMS: &[activity::ItemProps<'_>] = &[
    activity::ItemProps {
        icon: icons::PERSON_SIMPLE_RUN,
        title: "Running",
        subtitle: "30 May 2026 · 17:30",
        distance: Some("10.87 km"),
        duration: "51 min",
    },
    activity::ItemProps {
        icon: icons::PERSON_SIMPLE_RUN,
        title: "Running",
        subtitle: "28 May 2026 · 06:30",
        distance: Some("7.85 km"),
        duration: "43 min",
    },
    activity::ItemProps {
        icon: icons::BICYCLE,
        title: "Cycling",
        subtitle: "29 May 2026 · 07:45",
        distance: Some("18.42 km"),
        duration: "34 min",
    },
];

const METRICS: &[activity::MetricProps<'_>] = &[
    activity::MetricProps {
        label: "Distance",
        value: "10.87 km",
    },
    activity::MetricProps {
        label: "Active time",
        value: "51 min",
    },
    activity::MetricProps {
        label: "Average heart rate",
        value: "120 bpm",
    },
    activity::MetricProps {
        label: "Ascent",
        value: "46 m",
    },
];

const DETAIL: activity::DetailProps<'_> = activity::DetailProps {
    icon: icons::PERSON_SIMPLE_RUN,
    title: "Running",
    subtitle: "30 May 2026 · 17:30 · Mock Track-o-Matic 9000",
    metrics: METRICS,
    path: Some(garmin_ui::path::Props {
        segments: &[garmin_ui::path::Segment {
            points: &[
                garmin_ui::path::Point {
                    latitude: 50.0755,
                    longitude: 14.4378,
                },
                garmin_ui::path::Point {
                    latitude: 50.0810,
                    longitude: 14.4510,
                },
                garmin_ui::path::Point {
                    latitude: 50.0940,
                    longitude: 14.4290,
                },
            ],
        }],
        empty: "No recorded path",
        height: None,
    }),
    footer: Some("2 laps · 2 track points"),
};

#[derive(Clone, Copy)]
struct SceneProps {
    selected: Option<usize>,
    empty: bool,
    width: f32,
}

#[scene]
fn browser(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let selected = ctx.buttons("selection", &["first", "second", "none"], 0);
    let props = SceneProps {
        selected: (selected < 2).then_some(selected),
        empty: ctx.toggle("empty", false),
        width: ctx.slider("width", 960.0, 320.0, 1280.0, 1.0),
    };
    show_browser(ctx, ui, props);
}

#[scene(default)]
fn workspace(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Default,
        WorkspaceScene::COMPLETE,
    );
}

#[scene]
fn compact_workspace(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(620.0, 760.0),
        WorkspaceSlot::Compact,
        WorkspaceScene::COMPLETE,
    );
}

#[scene]
fn no_gps(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::NoGps,
        WorkspaceScene {
            recording: Some(RecordingKind::NoGps),
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn synchronized_hover(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Hover,
        WorkspaceScene {
            cursor: Some(activity::ActivityCursor {
                sample_index: Some(64),
                mode: activity::CursorMode::Hover,
            }),
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn pinned_cursor(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Pinned,
        WorkspaceScene {
            cursor: Some(activity::ActivityCursor {
                sample_index: Some(64),
                mode: activity::CursorMode::Pinned,
            }),
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn playback(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Playback,
        WorkspaceScene {
            cursor: Some(activity::ActivityCursor {
                sample_index: Some(32),
                mode: activity::CursorMode::Playback,
            }),
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn selected_lap(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Lap,
        WorkspaceScene {
            cursor: Some(activity::ActivityCursor {
                sample_index: Some(61),
                mode: activity::CursorMode::Pinned,
            }),
            selected_lap: Some(1),
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn missing_metrics(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::MissingMetrics,
        WorkspaceScene {
            recording: Some(RecordingKind::MissingMetrics),
            cursor: Some(activity::ActivityCursor {
                sample_index: Some(58),
                mode: activity::CursorMode::Pinned,
            }),
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn loading(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Loading,
        WorkspaceScene {
            recording: None,
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn provider_failure(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_workspace(
        ctx,
        ui,
        globals,
        egui::vec2(1_180.0, 760.0),
        WorkspaceSlot::Failure,
        WorkspaceScene {
            fail_tiles: true,
            ..WorkspaceScene::COMPLETE
        },
    );
}

#[scene]
fn device_fit_preview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(egui::vec2(1_800.0, 920.0)).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            FIT_PREVIEW.with_scene(
                0,
                || {
                    let runtime = gallery_map_runtime(false);
                    device_fit_preview::Preview::new(
                        DeviceBrowserTarget {
                            storage_id: "internal".to_owned(),
                            path: "Garmin/Activities/2026-09-15-ride.fit".into(),
                            kind: DeviceCatalogEntryKind::File,
                        },
                        DeviceFitPreview {
                            file_name: "2026-09-15-ride.fit".to_owned(),
                            activities: vec![DeviceFitPreviewActivity {
                                source: "Edge 850".to_owned(),
                                summary: summary(
                                    ActivitySport::Cycling,
                                    RECORDING_START_MILLISECONDS,
                                    43 * 60 * 1_000,
                                    16_800_000,
                                ),
                                recording: sample_recording(RecordingKind::Complete),
                            }],
                        },
                        &runtime,
                    )
                },
                |preview| {
                    let _ = preview.show(ui, &intl, false, UnitSystem::Metric);
                },
            );
        },
    );
}

fn show_workspace(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    size: egui::Vec2,
    slot: WorkspaceSlot,
    scene: WorkspaceScene,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(size).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            let recording = scene.recording.map(sample_recording);
            let presentations = sample_presentations(&intl);
            let items = presentations
                .iter()
                .map(activity::Presentation::item_props)
                .collect::<Vec<_>>();
            WORKSPACES.with_scene(
                slot as usize,
                || {
                    let runtime = gallery_map_runtime(scene.fail_tiles);
                    (activity::Workspace::new(&runtime), 0)
                },
                |(workspace, selected)| {
                    if let Some(cursor) = scene.cursor {
                        workspace.set_cursor(cursor);
                    }
                    if let Some(selected_lap) = scene.selected_lap {
                        workspace.set_selected_lap(Some(selected_lap));
                    }
                    if let Some(activity::Action::Select(index)) = workspace.show(
                        ui,
                        &intl,
                        &activity::WorkspaceProps {
                            items: &items,
                            presentations: &presentations,
                            selected: Some(*selected),
                            recording: recording.as_ref(),
                            recording_key: scene.recording.map(|kind| match kind {
                                RecordingKind::Complete => "complete",
                                RecordingKind::MissingMetrics => "missing-metrics",
                                RecordingKind::NoGps => "no-gps",
                            }),
                            units: UnitSystem::Metric,
                            empty_list: "No activities yet",
                            empty_detail: "Select an activity",
                            no_route: "No recorded route",
                        },
                    ) {
                        *selected = index;
                    }
                },
            );
        },
    );
}

fn gallery_map_runtime(fail_tiles: bool) -> activity::map_runtime::MapRuntimeHandle {
    activity::map_runtime::MapRuntimeHandle::new(
        GalleryMapBackend { fail_tiles },
        activity::map_runtime::Renderer::software(),
    )
}

#[derive(Clone, Copy)]
struct GalleryMapBackend {
    fail_tiles: bool,
}

impl activity::map_runtime::Backend for GalleryMapBackend {
    fn submit(&self, task: activity::map_runtime::TileTask) {
        let request = task.coordinates();
        let result = if self.fail_tiles {
            Err("gallery provider unavailable".to_owned())
        } else {
            Ok(gallery_vector_tile(request))
        };
        task.complete_encoded(result);
    }
}

fn gallery_vector_tile(request: activity::map_runtime::TileCoordinates) -> Vec<u8> {
    let drift = i32::try_from((request.x ^ request.y) % 5).unwrap_or_default() * 90;
    let forest = tile_feature(
        3,
        &polygon_geometry(&[
            (0, 0),
            (2_250 + drift, 0),
            (2_100 + drift, 900),
            (1_750, 1_650),
            (850, 2_100),
            (0, 1_850),
        ]),
        true,
    );
    let water = tile_feature(
        3,
        &polygon_geometry(&[
            (3_150 - drift, 0),
            (4_096, 0),
            (4_096, 4_096),
            (3_350 + drift, 4_096),
            (3_200, 3_250),
            (3_500 - drift, 2_350),
            (3_230, 1_450),
            (3_420 - drift, 650),
        ]),
        false,
    );
    let roads = [
        tile_feature(
            2,
            &line_geometry(&[
                (0, 3_150 - drift),
                (900, 2_720),
                (1_850, 2_520 + drift),
                (2_850, 1_950),
                (4_096, 1_700 + drift),
            ]),
            true,
        ),
        tile_feature(
            2,
            &line_geometry(&[
                (1_050 + drift, 0),
                (1_250, 900),
                (1_600, 1_900),
                (1_520 + drift, 3_000),
                (1_850, 4_096),
            ]),
            true,
        ),
    ];
    let buildings = [
        tile_feature(
            3,
            &polygon_geometry(&[
                (1_900, 2_850),
                (2_300, 2_850),
                (2_300, 3_200),
                (1_900, 3_200),
            ]),
            false,
        ),
        tile_feature(
            3,
            &polygon_geometry(&[
                (2_420, 2_650),
                (2_900, 2_650),
                (2_900, 3_050),
                (2_420, 3_050),
            ]),
            false,
        ),
        tile_feature(
            3,
            &polygon_geometry(&[(700, 2_950), (1_080, 2_950), (1_080, 3_350), (700, 3_350)]),
            false,
        ),
    ];

    let layers = [
        tile_layer("landcover", &[forest], Some(("class", "forest"))),
        tile_layer("water", &[water], None),
        tile_layer("transportation", &roads, Some(("class", "primary"))),
        tile_layer("building", &buildings, None),
    ];
    let mut tile = Vec::new();
    for layer in &layers {
        push_bytes_field(&mut tile, 3, layer);
    }
    tile
}

fn tile_layer(name: &str, features: &[Vec<u8>], property: Option<(&str, &str)>) -> Vec<u8> {
    let mut layer = Vec::new();
    push_bytes_field(&mut layer, 1, name.as_bytes());
    for feature in features {
        push_bytes_field(&mut layer, 2, feature);
    }
    if let Some((key, value)) = property {
        push_bytes_field(&mut layer, 3, key.as_bytes());
        let mut encoded_value = Vec::new();
        push_bytes_field(&mut encoded_value, 1, value.as_bytes());
        push_bytes_field(&mut layer, 4, &encoded_value);
    }
    push_varint_field(&mut layer, 5, 4_096);
    push_varint_field(&mut layer, 15, 2);
    layer
}

fn tile_feature(geometry_type: u64, geometry: &[u32], tagged: bool) -> Vec<u8> {
    let mut feature = Vec::new();
    if tagged {
        push_packed_field(&mut feature, 2, &[0, 0]);
    }
    push_varint_field(&mut feature, 3, geometry_type);
    push_packed_field(&mut feature, 4, geometry);
    feature
}

fn line_geometry(points: &[(i32, i32)]) -> Vec<u32> {
    geometry(points, false)
}

fn polygon_geometry(points: &[(i32, i32)]) -> Vec<u32> {
    geometry(points, true)
}

fn geometry(points: &[(i32, i32)], closed: bool) -> Vec<u32> {
    let Some(&(first_x, first_y)) = points.first() else {
        return Vec::new();
    };
    let mut encoded = vec![9, zigzag(first_x), zigzag(first_y)];
    if points.len() > 1 {
        let count = u32::try_from(points.len() - 1).unwrap_or_default();
        encoded.push((count << 3) | 2);
        let mut previous = (first_x, first_y);
        for &(x, y) in &points[1..] {
            encoded.push(zigzag(x - previous.0));
            encoded.push(zigzag(y - previous.1));
            previous = (x, y);
        }
    }
    if closed {
        encoded.push(15);
    }
    encoded
}

fn zigzag(value: i32) -> u32 {
    u32::try_from((i64::from(value) << 1) ^ (i64::from(value) >> 63)).unwrap_or_default()
}

fn push_packed_field(output: &mut Vec<u8>, field: u64, values: &[u32]) {
    let mut packed = Vec::new();
    for &value in values {
        push_varint(&mut packed, u64::from(value));
    }
    push_bytes_field(output, field, &packed);
}

fn push_bytes_field(output: &mut Vec<u8>, field: u64, value: &[u8]) {
    push_varint(output, (field << 3) | 2);
    push_varint(output, u64::try_from(value.len()).unwrap_or_default());
    output.extend_from_slice(value);
}

fn push_varint_field(output: &mut Vec<u8>, field: u64, value: u64) {
    push_varint(output, field << 3);
    push_varint(output, value);
}

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        let byte = u8::try_from(value & 0x7f).unwrap_or_default();
        output.push(byte | 0x80);
        value >>= 7;
    }
    output.push(u8::try_from(value).unwrap_or_default());
}

fn sample_presentations(intl: &garmin_i18n::Intl) -> Vec<activity::Presentation> {
    [
        (
            ActivitySport::Cycling,
            RECORDING_START_MILLISECONDS,
            43 * 60 * 1_000,
            16_800_000,
            "Edge 850",
        ),
        (
            ActivitySport::Running,
            RECORDING_START_MILLISECONDS + 86_400_000,
            51 * 60 * 1_000,
            10_870_000,
            "Forerunner",
        ),
        (
            ActivitySport::Cycling,
            RECORDING_START_MILLISECONDS + 172_800_000,
            34 * 60 * 1_000,
            18_420_000,
            "Edge 850",
        ),
    ]
    .into_iter()
    .map(|(sport, start, duration, distance, source)| {
        activity::Presentation::from_summary(
            summary(sport, start, duration, distance),
            source,
            intl,
            UnitSystem::Metric,
        )
    })
    .collect()
}

fn summary(
    sport: ActivitySport,
    start_milliseconds: i64,
    duration_milliseconds: u64,
    distance_millimeters: u64,
) -> ActivitySummary {
    let start = Timestamp::from_unix_milliseconds(start_milliseconds)
        .expect("the fixture start timestamp is valid");
    let end = Timestamp::from_unix_milliseconds(
        start_milliseconds
            + i64::try_from(duration_milliseconds).expect("the fixture duration fits i64"),
    )
    .expect("the fixture end timestamp is valid");
    let time = TimeRange::from_parts(start, end).expect("the fixture time range is ordered");
    let duration = ActivityDuration::from_milliseconds(duration_milliseconds);
    let totals = ActivityTotals::from_parts(
        duration,
        duration,
        Some(Distance::from_millimeters(distance_millimeters)),
        None,
        Some(Distance::from_millimeters(130_000)),
        Some(Distance::from_millimeters(128_000)),
    )
    .expect("the fixture totals are internally consistent");
    ActivitySummary::from_parts(sport, time, totals, ActivityMetrics::default())
}

fn sample_recording(kind: RecordingKind) -> ActivityRecordingSnapshot {
    let sample_count = 121_u64;
    let duration = 43 * 60 * 1_000_u64;
    let samples = (0..sample_count)
        .map(|index| sample(kind, index, sample_count, duration))
        .collect::<Vec<_>>();
    let laps = sample_laps(&samples, duration);
    ActivityRecordingSnapshot {
        laps,
        samples,
        timer_events: vec![
            ActivityTimerEventSnapshot {
                timestamp: Timestamp::from_unix_milliseconds(
                    RECORDING_START_MILLISECONDS + 1_280_000,
                )
                .expect("the fixture stop timestamp is valid"),
                state: ActivityTimerStateSnapshot::Stopped,
            },
            ActivityTimerEventSnapshot {
                timestamp: Timestamp::from_unix_milliseconds(
                    RECORDING_START_MILLISECONDS + 1_320_000,
                )
                .expect("the fixture resume timestamp is valid"),
                state: ActivityTimerStateSnapshot::Running,
            },
        ],
    }
}

fn sample(
    kind: RecordingKind,
    index: u64,
    sample_count: u64,
    duration: u64,
) -> ActivitySampleSnapshot {
    let fraction = f64::from(u32::try_from(index).expect("the fixture index fits u32"))
        / f64::from(u32::try_from(sample_count - 1).expect("the fixture sample count fits u32"));
    let timestamp = Timestamp::from_unix_milliseconds(
        RECORDING_START_MILLISECONDS
            + i64::try_from(index * duration / (sample_count - 1))
                .expect("the fixture sample timestamp fits i64"),
    )
    .expect("the fixture sample timestamp is valid");
    let angle = fraction * std::f64::consts::TAU;
    let telemetry = sample_telemetry(fraction, angle);
    let coordinate = sample_coordinate(kind, index, angle);
    ActivitySampleSnapshot {
        timestamp,
        coordinate,
        elevation_meters: (kind != RecordingKind::MissingMetrics || !index.is_multiple_of(29))
            .then_some(telemetry.elevation),
        distance: Some(Distance::from_millimeters(
            index * 16_800_000 / (sample_count - 1),
        )),
        speed: Some(Speed::from_millimeters_per_second(rounded_u32(
            telemetry.speed * 1_000.0,
        ))),
        heart_rate: (kind != RecordingKind::MissingMetrics || !index.is_multiple_of(7)).then(
            || {
                HeartRate::from_beats_per_minute(
                    u16::try_from(rounded_u32(telemetry.heart_rate))
                        .expect("the fixture heart rate fits u16"),
                )
            },
        ),
        cadence: (kind != RecordingKind::MissingMetrics || !index.is_multiple_of(11)).then(|| {
            Cadence::from_revolutions_per_minute(telemetry.cadence)
                .expect("the fixture cadence is valid")
        }),
        power: (kind != RecordingKind::MissingMetrics || !index.is_multiple_of(13))
            .then(|| Power::from_watts(rounded_u32(telemetry.power))),
        temperature_millicelsius: (kind != RecordingKind::MissingMetrics
            || !index.is_multiple_of(17))
        .then_some(telemetry.temperature_millicelsius),
    }
}

struct SampleTelemetry {
    elevation: f64,
    speed: f64,
    heart_rate: f64,
    cadence: f64,
    power: f64,
    temperature_millicelsius: i32,
}

fn sample_telemetry(fraction: f64, angle: f64) -> SampleTelemetry {
    let climb = bell(fraction, 0.56, 0.11);
    let elevation = bell(fraction, 0.78, 0.05).mul_add(
        7.0,
        climb.mul_add(
            18.0,
            (angle * 7.0)
                .sin()
                .mul_add(1.3, (angle * 2.0).sin().mul_add(2.4, 12.0)),
        ),
    );
    let speed_meters_per_second = bell(fraction, 0.28, 0.04)
        .mul_add(
            1.2,
            climb.mul_add(
                -1.5,
                (angle * 13.0)
                    .sin()
                    .mul_add(0.35, (angle * 5.0).sin().mul_add(0.7, 6.5)),
            ),
        )
        .clamp(3.0, 11.0);
    let heart_rate = climb.mul_add(
        10.0,
        (angle * 3.0)
            .sin()
            .mul_add(4.0, fraction.mul_add(18.0, 118.0)),
    );
    let cadence = climb
        .mul_add(
            -8.0,
            (angle * 11.0)
                .sin()
                .mul_add(2.0, (angle * 4.0).sin().mul_add(6.0, 82.0)),
        )
        .clamp(55.0, 105.0);
    let power = climb.mul_add(
        65.0,
        (angle * 17.0)
            .sin()
            .mul_add(18.0, (angle * 5.0).sin().mul_add(35.0, 175.0)),
    );
    let temperature_millicelsius = i32::try_from(rounded_u32(
        (angle * 2.0)
            .sin()
            .mul_add(0.25, fraction.mul_add(-1.4, 19.0))
            * 1_000.0,
    ))
    .expect("the fixture temperature fits i32");
    SampleTelemetry {
        elevation,
        speed: speed_meters_per_second,
        heart_rate,
        cadence,
        power,
        temperature_millicelsius,
    }
}

fn sample_coordinate(kind: RecordingKind, index: u64, angle: f64) -> Option<Coordinate> {
    (kind != RecordingKind::NoGps && index != 62).then(|| {
        Coordinate::from_parts(
            Latitude::from_degrees(
                angle
                    .sin()
                    .mul_add(0.017, (angle * 3.0).sin().mul_add(0.003, 60.1708)),
            )
            .expect("the fixture latitude is valid"),
            Longitude::from_degrees(
                angle
                    .cos()
                    .mul_add(0.032, (angle * 2.0).sin().mul_add(0.005, 24.9375)),
            )
            .expect("the fixture longitude is valid"),
        )
    })
}

fn bell(value: f64, center: f64, width: f64) -> f64 {
    (-((value - center) / width).powi(2)).exp()
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "gallery telemetry is clamped to the target's complete non-negative range"
)]
fn rounded_u32(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

fn sample_laps(samples: &[ActivitySampleSnapshot], duration: u64) -> Vec<ActivityLapSnapshot> {
    let halfway = duration / 2;
    let first_time = TimeRange::from_parts(
        samples
            .first()
            .expect("the fixture always contains samples")
            .timestamp,
        Timestamp::from_unix_milliseconds(
            RECORDING_START_MILLISECONDS
                + i64::try_from(halfway).expect("the fixture lap timestamp fits i64"),
        )
        .expect("the fixture lap timestamp is valid"),
    )
    .expect("the first fixture lap is ordered");
    let second_time = TimeRange::from_parts(
        first_time.end(),
        samples
            .last()
            .expect("the fixture always contains samples")
            .timestamp,
    )
    .expect("the second fixture lap is ordered");
    let lap_totals = |distance| {
        ActivityTotals::from_parts(
            ActivityDuration::from_milliseconds(halfway),
            ActivityDuration::from_milliseconds(halfway - 20_000),
            Some(Distance::from_millimeters(distance)),
            None,
            None,
            None,
        )
        .expect("the fixture lap totals are internally consistent")
    };
    vec![
        ActivityLapSnapshot {
            time: first_time,
            totals: lap_totals(8_350_000),
            metrics: ActivityMetrics::default(),
        },
        ActivityLapSnapshot {
            time: second_time,
            totals: lap_totals(8_450_000),
            metrics: ActivityMetrics::default(),
        },
    ]
}

#[scene]
fn list(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        let _ = activity::list(
            ui,
            &activity::ListProps {
                items: ITEMS,
                selected: Some(0),
                empty: "No activities yet",
            },
        );
    });
}

#[scene]
fn detail(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(560.0);
        activity::detail(ui, &DETAIL);
    });
}

#[scene]
fn empty(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_browser(
        ctx,
        ui,
        SceneProps {
            selected: None,
            empty: true,
            width: 720.0,
        },
    );
}

fn show_browser(ctx: &mut SceneCtx<'_>, ui: &mut Ui, props: SceneProps) {
    stage!(ctx, ui, |ui| {
        ui.set_width(props.width);
        let items = if props.empty { &ITEMS[..0] } else { ITEMS };
        let _ = activity::browser(
            ui,
            &activity::BrowserProps {
                list: activity::ListProps {
                    items,
                    selected: props.selected,
                    empty: "No activities yet",
                },
                detail: if props.empty {
                    None
                } else {
                    props.selected.map(|_| &DETAIL)
                },
                empty_detail: "Select an activity",
            },
        );
    });
}
