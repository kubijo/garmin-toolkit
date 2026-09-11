use gallery::prelude::*;
use garmin_ui::{activity, icons};

scene_meta! { title: "Desktop / Activities" }

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
    subtitle: "30 May 2026 · 17:30 · Synthetic tracker",
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

#[scene(default)]
fn browser(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let selected = ctx.buttons("selection", &["first", "second", "none"], 0);
    let props = SceneProps {
        selected: (selected < 2).then_some(selected),
        empty: ctx.toggle("empty", false),
        width: ctx.slider("width", 960.0, 320.0, 1280.0, 1.0),
    };
    show_browser(ctx, ui, props);
}

#[scene]
fn list(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
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
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        ui.set_width(560.0);
        activity::detail(ui, &DETAIL);
    });
}

#[scene]
fn empty(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
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
