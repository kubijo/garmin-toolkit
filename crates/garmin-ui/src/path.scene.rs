use crate::fixture_coordinate;
use gallery::prelude::*;
use garmin_model::route::Coordinate;
use garmin_ui::path;

scene_meta! { title: "Components / Path preview" }

const FIRST: &[Coordinate] = &[
    fixture_coordinate(50.0755, 14.4378),
    fixture_coordinate(50.0810, 14.4510),
    fixture_coordinate(50.0900, 14.4480),
    fixture_coordinate(50.0940, 14.4290),
    fixture_coordinate(50.0840, 14.4190),
];
const SECOND: &[Coordinate] = &[
    fixture_coordinate(50.0840, 14.4190),
    fixture_coordinate(50.0780, 14.4250),
];
const SEGMENTS: &[path::Segment<'_>] = &[
    path::Segment { points: FIRST },
    path::Segment { points: SECOND },
];

#[scene(default)]
fn path_preview(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(560.0);
        path::preview(
            ui,
            &path::Props {
                segments: SEGMENTS,
                empty: "No recorded path",
                height: None,
            },
        );
    });
}

#[scene]
fn empty(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.set_width(560.0);
        path::preview(
            ui,
            &path::Props {
                segments: &[],
                empty: "No recorded path",
                height: None,
            },
        );
    });
}
