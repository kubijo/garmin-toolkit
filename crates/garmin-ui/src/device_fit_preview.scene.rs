use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::{
    identity::UnitSystem,
    route::{Coordinate, Latitude, Longitude, RoutePoint, RouteSport},
};
use garmin_service_api::{
    DeviceBrowserTarget, DeviceCatalogEntryKind, DeviceFitPreview, DeviceFitPreviewContent,
    DeviceFitPreviewCourse,
};
use garmin_ui::{activity::map_runtime, device_fit_preview};

scene_meta! { title: "Components / FIT course preview" }

thread_local! {
    static PREVIEW: crate::SceneState<device_fit_preview::Preview, 2> = const { crate::SceneState::empty() };
}

#[scene(default)]
fn course(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let empty = ctx.toggle("empty", false);
    let width = ctx.slider("width", 1120.0, 480.0, 1600.0, 8.0);
    stage!(ctx, ui, globals.stage((width, 880.0)), |ui| {
        PREVIEW.with_scene(
            usize::from(empty),
            || {
                let runtime =
                    map_runtime::MapRuntimeHandle::new(NoTiles, map_runtime::Renderer::software())
                        .on_ui_thread();
                let points = if empty {
                    Vec::new()
                } else {
                    [(50.0, 14.0), (50.001, 14.002), (50.002, 14.001)]
                        .into_iter()
                        .map(|(latitude, longitude)| {
                            RoutePoint::from_parts(
                                Coordinate::from_parts(
                                    Latitude::from_degrees(latitude).expect("synthetic latitude"),
                                    Longitude::from_degrees(longitude)
                                        .expect("synthetic longitude"),
                                ),
                                None,
                            )
                        })
                        .collect()
                };
                device_fit_preview::Preview::new(
                    DeviceBrowserTarget {
                        storage_id: "synthetic".to_owned(),
                        path: "Garmin/Courses/loop.fit".into(),
                        kind: DeviceCatalogEntryKind::File,
                    },
                    DeviceFitPreview {
                        file_name: "loop.fit".to_owned(),
                        content: DeviceFitPreviewContent::Course(DeviceFitPreviewCourse {
                            name: Some("Synthetic canal loop".to_owned()),
                            sport: Some(RouteSport::Cycling),
                            points,
                            cue_count: 0,
                        }),
                    },
                    &runtime,
                )
                .with_presentation(garmin_ui::modal::Presentation::Contained)
            },
            |preview| {
                let theme = ui.ctx().theme();
                let previous_style = ui.ctx().style_of(theme);
                ui.ctx().set_style_of(theme, ui.style().clone());
                let _ = preview.show(
                    ui,
                    &globals.intl(),
                    false,
                    UnitSystem::Metric,
                    jiff::civil::date(2026, 3, 1),
                );
                ui.ctx().set_style_of(theme, previous_style);
            },
        );
    });
}

struct NoTiles;

impl map_runtime::Backend for NoTiles {
    fn fetch(&self, _: map_runtime::TileCoordinates, reply: map_runtime::TileReply) {
        reply(Err("Synthetic gallery has no basemap".to_owned()));
    }
}
