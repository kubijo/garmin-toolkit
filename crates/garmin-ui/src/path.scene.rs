use gallery::prelude::*;
use garmin_ui::path;

scene_meta! { title: "Components / Data display / Path preview" }

const FIRST: &[path::Point] = &[
    path::Point {
        latitude: 50.0755,
        longitude: 14.4378,
    },
    path::Point {
        latitude: 50.0810,
        longitude: 14.4510,
    },
    path::Point {
        latitude: 50.0900,
        longitude: 14.4480,
    },
    path::Point {
        latitude: 50.0940,
        longitude: 14.4290,
    },
    path::Point {
        latitude: 50.0840,
        longitude: 14.4190,
    },
];
const SECOND: &[path::Point] = &[
    path::Point {
        latitude: 50.0840,
        longitude: 14.4190,
    },
    path::Point {
        latitude: 50.0780,
        longitude: 14.4250,
    },
];
const SEGMENTS: &[path::Segment<'_>] = &[
    path::Segment { points: FIRST },
    path::Segment { points: SECOND },
];

#[scene(default)]
fn path_preview(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
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
    garmin_ui::theme::apply(ui.style_mut());
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
