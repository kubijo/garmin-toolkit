use egui::{Event, FullOutput, RawInput, Rect, pos2, vec2};
use garmin_i18n::{Language, Translations};
use garmin_model::{
    artifact::{ArtifactDigest, ArtifactId, ByteCount},
    route::{Coordinate, Latitude, Longitude, RoutePlanId, RoutePlanRevisionId, RoutePoint},
};
use garmin_service_api::routes::{RouteSource, RouteSummary};

use super::*;

struct NoTiles;
impl crate::activity::map_runtime::Backend for NoTiles {
    fn fetch(
        &self,
        _: crate::activity::map_runtime::TileCoordinates,
        reply: crate::activity::map_runtime::TileReply,
    ) {
        reply(Err("offline layout test".into()));
    }
}

fn route() -> RouteSummary {
    RouteSummary {
        id: RoutePlanId::new_v4(),
        revision: RoutePlanRevisionId::new_v4(),
        name: "A very long walking route name that must fit even in a narrow window"
            .parse()
            .unwrap(),
        sport: RouteSport::Walking,
        created_at: "2026-10-01T12:00:00Z".parse().unwrap(),
        geometry: true,
        point_count: 2,
    }
}

fn frame(
    ctx: &egui::Context,
    intl: &Intl,
    view: &mut Workspace,
    state: &mut State,
    width: f32,
    events: Vec<Event>,
) -> (FullOutput, Option<Action>) {
    let mut action = None;
    let output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 1000.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            action = view.show(ui, intl, state);
            assert!(
                ui.min_rect().right() <= width,
                "route layout overflowed {width}: {:?}",
                ui.min_rect()
            );
        },
    );
    (output, action)
}

fn bounds(output: &FullOutput, target: &str) -> egui::accesskit::Rect {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.author_id() == Some(target))
        .unwrap_or_else(|| panic!("missing {target}"))
        .1
        .bounds()
        .unwrap()
}

#[test]
fn library_and_detail_fit_narrow_windows_in_both_languages() {
    let translations = Translations::bundled().unwrap();
    for language in [Language::English, Language::Czech] {
        let intl = translations.formatter(language).unwrap();
        for width in [320.0, 480.0, 800.0, 1600.0] {
            let ctx = egui::Context::default();
            crate::install(&ctx);
            ctx.enable_accesskit();
            let runtime =
                MapRuntimeHandle::new(NoTiles, crate::activity::map_runtime::Renderer::software());
            let mut view = Workspace::new(&runtime);
            let route = route();
            let mut state = State {
                routes: vec![route.clone()],
                ..State::default()
            };
            let (output, _) = frame(&ctx, &intl, &mut view, &mut state, width, vec![]);
            let row = bounds(&output, &format!("routes.open.{}", route.id));
            assert!(row.x1 - row.x0 <= 1120.0);
            assert!(row.y1 - row.y0 >= 64.0);
            output.drop_without_applying_deltas();
            state.detail = Some(route);
            state.source = Some(RouteSource {
                artifact: ArtifactId::new_v4(),
                digest: ArtifactDigest::from_bytes(b"gpx"),
                byte_count: ByteCount::from_u64(3),
                name: "walk.gpx".into(),
            });
            state.points = [(50.0755, 14.4378), (50.0810, 14.4510)]
                .into_iter()
                .map(|(lat, lon)| {
                    RoutePoint::from_parts(
                        Coordinate::from_parts(
                            Latitude::from_degrees(lat).unwrap(),
                            Longitude::from_degrees(lon).unwrap(),
                        ),
                        None,
                    )
                })
                .collect();
            state.points_ready = true;
            let (output, _) = frame(&ctx, &intl, &mut view, &mut state, width, vec![]);
            let back = bounds(&output, "routes.back");
            let generate = bounds(&output, "routes.generate");
            let source = bounds(&output, "routes.source");
            let map = bounds(&output, "map");
            let title = bounds(&output, "routes.title");
            let sport = bounds(&output, "routes.sport");
            assert!(back.x1 <= title.x0 && title.x1 <= sport.x0);
            assert!(sport.y0 < title.y1 && title.y0 < sport.y1);
            assert!(generate.y0 >= map.y1 && source.y1 <= map.y0);
            assert!(map.y1 - map.y0 >= 256.0);
            if width >= 1600.0 {
                assert!(map.y1 - map.y0 >= 480.0);
                assert!(source.y0 < back.y1 && back.y0 < source.y1);
                assert!(
                    (source.x1 - map.x1).abs() <= 1.0,
                    "actions must align with the map's right edge"
                );
            }
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn current_course_survives_history_paging_and_hides_generation() {
    use garmin_model::{
        route::{CourseGenerationId, CourseGenerationOperationId},
        value::ComponentVersion,
    };
    use garmin_service_api::routes::{CourseVersion, RouteReply};

    let ctx = egui::Context::default();
    crate::install(&ctx);
    ctx.enable_accesskit();
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let runtime =
        MapRuntimeHandle::new(NoTiles, crate::activity::map_runtime::Renderer::software());
    let mut view = Workspace::new(&runtime);
    let route = route();
    let current = CourseVersion {
        id: CourseGenerationId::new_v4(),
        operation: CourseGenerationOperationId::new_v4(),
        revision: route.revision,
        artifact: ArtifactId::new_v4(),
        version: 2.try_into().unwrap(),
        serial: 2.try_into().unwrap(),
        encoder: ComponentVersion::from_parts("fixture", "2.0.0".parse().unwrap()).unwrap(),
        current_encoder: true,
        generated_at: route.created_at,
        byte_count: ByteCount::from_u64(512),
        digest: ArtifactDigest::from_bytes(b"current"),
    };
    let mut older = current.clone();
    older.id = CourseGenerationId::new_v4();
    older.version = 1.try_into().unwrap();
    older.current_encoder = false;
    let mut state = State {
        detail: Some(route),
        versions: vec![current.clone()],
        ..State::default()
    };
    state.queue(RouteRequest::Versions {
        revision: current.revision,
        offset: 1,
    });
    assert!(state.take_request().is_some());
    state.accept(RouteReply::Versions {
        items: vec![older.clone()],
        next: None,
    });
    assert_eq!(state.versions, vec![current.clone(), older.clone()]);
    let (output, _) = frame(&ctx, &intl, &mut view, &mut state, 1200.0, vec![]);
    let nodes = &output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes;
    assert!(
        !nodes
            .iter()
            .any(|(_, node)| node.author_id() == Some("routes.generate"))
    );
    assert!(!nodes.iter().any(
        |(_, node)| node.author_id() == Some(format!("routes.download.{}", older.id).as_str())
    ));
    bounds(&output, &format!("routes.download.{}", current.id));
    bounds(&output, "routes.history");
    output.drop_without_applying_deltas();
    state.versions.remove(0);
    let (output, _) = frame(&ctx, &intl, &mut view, &mut state, 1200.0, vec![]);
    bounds(&output, "routes.generate");
    bounds(&output, "routes.history");
    output.drop_without_applying_deltas();
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "AccessKit exposes egui f32 coordinates as f64"
)]
fn route_rows_have_pointer_and_keyboard_affordances() {
    let ctx = egui::Context::default();
    crate::install(&ctx);
    ctx.enable_accesskit();
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let runtime =
        MapRuntimeHandle::new(NoTiles, crate::activity::map_runtime::Renderer::software());
    let mut view = Workspace::new(&runtime);
    let route = route();
    let id = route.id;
    let mut state = State {
        routes: vec![route],
        ..State::default()
    };
    let (output, _) = frame(&ctx, &intl, &mut view, &mut state, 480.0, vec![]);
    let rect = bounds(&output, &format!("routes.open.{id}"));
    let position = pos2(
        rect.x0.midpoint(rect.x1) as f32,
        rect.y0.midpoint(rect.y1) as f32,
    );
    output.drop_without_applying_deltas();
    let (output, _) = frame(
        &ctx,
        &intl,
        &mut view,
        &mut state,
        480.0,
        vec![Event::PointerMoved(position)],
    );
    assert_eq!(
        output.platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    output.drop_without_applying_deltas();
    let key = |key| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    for _ in 0..2 {
        let (output, _) = frame(
            &ctx,
            &intl,
            &mut view,
            &mut state,
            480.0,
            vec![key(egui::Key::Tab)],
        );
        output.drop_without_applying_deltas();
    }
    let (output, action) = frame(
        &ctx,
        &intl,
        &mut view,
        &mut state,
        480.0,
        vec![key(egui::Key::Enter)],
    );
    assert!(matches!(action, Some(Action::Request(RouteRequest::Detail { plan })) if plan == id));
    output.drop_without_applying_deltas();
}

#[test]
fn import_form_and_course_versions_fit_narrow_windows() {
    use garmin_model::{
        artifact::AcquisitionOperationId,
        route::{CourseGenerationId, CourseGenerationOperationId, RouteCandidateSource},
        value::ComponentVersion,
    };
    use garmin_service_api::routes::{CourseVersion, GpxCandidate, GpxUpload, GpxUploadPhase};

    let translations = Translations::bundled().unwrap();
    for language in [Language::English, Language::Czech] {
        let intl = translations.formatter(language).unwrap();
        for width in [320.0, 800.0, 1200.0] {
            let ctx = egui::Context::default();
            crate::install(&ctx);
            ctx.enable_accesskit();
            let runtime =
                MapRuntimeHandle::new(NoTiles, crate::activity::map_runtime::Renderer::software());
            let mut view = Workspace::new(&runtime);
            let route = route();
            let candidate = RouteCandidateSource::TrackSegment {
                track: 0,
                segment: 0,
            };
            let mut state = State {
                upload: Some(GpxUpload {
                    operation: AcquisitionOperationId::new_v4(),
                    file_name: "A long file name for a walking route.gpx".into(),
                    received: ByteCount::from_u64(128),
                    total: ByteCount::from_u64(128),
                    phase: GpxUploadPhase::Review {
                        digest: ArtifactDigest::from_bytes(b"fixture"),
                        candidates: 1,
                        rejected: 0,
                    },
                }),
                candidates: vec![GpxCandidate {
                    source: candidate,
                    suggested_name: Some(route.name.clone()),
                    geometry: true,
                    point_count: 2,
                }],
                candidate: Some(candidate),
                name: route.name.to_string(),
                ..State::default()
            };
            let (output, _) = frame(&ctx, &intl, &mut view, &mut state, width, vec![]);
            assert!(bounds(&output, "routes.name").x1 <= f64::from(width));
            let name_label = bounds(&output, "routes.name.label");
            let type_label = bounds(&output, "routes.type.label");
            let walking = bounds(&output, "routes.sport.walking");
            assert!(type_label.y1 <= walking.y0);
            if width >= 1200.0 {
                assert!((name_label.y0 - type_label.y0).abs() <= 1.0);
            } else {
                assert!(bounds(&output, "routes.name").y1 <= type_label.y0);
            }
            assert!(bounds(&output, "routes.cancel").y1 <= name_label.y0);
            output.drop_without_applying_deltas();
            state.upload = None;
            state.versions = vec![CourseVersion {
                id: CourseGenerationId::new_v4(),
                operation: CourseGenerationOperationId::new_v4(),
                revision: route.revision,
                artifact: ArtifactId::new_v4(),
                version: 1.try_into().unwrap(),
                serial: 1.try_into().unwrap(),
                encoder: ComponentVersion::from_parts("fixture", "1.0.0".parse().unwrap()).unwrap(),
                current_encoder: true,
                generated_at: route.created_at,
                byte_count: ByteCount::from_u64(512),
                digest: ArtifactDigest::from_bytes(b"fixture"),
            }];
            state.detail = Some(route);
            let (output, _) = frame(&ctx, &intl, &mut view, &mut state, width, vec![]);
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "AccessKit exposes egui f32 coordinates as f64"
)]
fn deleting_a_course_requires_confirmation_and_cancel_preserves_it() {
    use garmin_model::{
        route::{CourseGenerationId, CourseGenerationOperationId},
        value::ComponentVersion,
    };
    use garmin_service_api::routes::CourseVersion;

    let ctx = egui::Context::default();
    crate::install(&ctx);
    ctx.enable_accesskit();
    let intl = Translations::bundled()
        .unwrap()
        .formatter(Language::English)
        .unwrap();
    let runtime =
        MapRuntimeHandle::new(NoTiles, crate::activity::map_runtime::Renderer::software());
    let mut view = Workspace::new(&runtime);
    let route = route();
    let version = CourseVersion {
        id: CourseGenerationId::new_v4(),
        operation: CourseGenerationOperationId::new_v4(),
        revision: route.revision,
        artifact: ArtifactId::new_v4(),
        version: 1.try_into().unwrap(),
        serial: 1.try_into().unwrap(),
        encoder: ComponentVersion::from_parts("fixture", "1.0.0".parse().unwrap()).unwrap(),
        current_encoder: true,
        generated_at: route.created_at,
        byte_count: ByteCount::from_u64(512),
        digest: ArtifactDigest::from_bytes(b"fixture"),
    };
    let mut state = State {
        detail: Some(route),
        versions: vec![version.clone()],
        ..State::default()
    };
    for confirm in [false, true] {
        view.deleting = Some(version.clone());
        // Dialog sizing settles on the first frame before clicking its real bounds.
        frame(&ctx, &intl, &mut view, &mut state, 800.0, vec![])
            .0
            .drop_without_applying_deltas();
        let (output, action) = frame(&ctx, &intl, &mut view, &mut state, 800.0, vec![]);
        assert!(action.is_none());
        let target = if confirm {
            "routes.delete.confirm"
        } else {
            "routes.delete.cancel"
        };
        let rect = bounds(&output, target);
        let pos = pos2(
            rect.x0.midpoint(rect.x1) as f32,
            rect.y0.midpoint(rect.y1) as f32,
        );
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            let events = vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let (output, action) = frame(&ctx, &intl, &mut view, &mut state, 800.0, events);
            if confirm && !pressed {
                assert!(
                    matches!(action, Some(Action::Request(RouteRequest::DeleteCourse { generation, revision })) if generation == version.id && revision == version.revision)
                );
            } else {
                assert!(action.is_none());
            }
            output.drop_without_applying_deltas();
        }
        assert!(view.deleting.is_none());
        assert_eq!(state.versions, vec![version.clone()]);
    }
}
