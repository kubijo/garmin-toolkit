use gallery::prelude::*;
use garmin_color::swatch;
use garmin_ui::{icons, profile, shell};

scene_meta! { title: "Application / Shell" }

const PRIMARY_DESTINATIONS: &[shell::Destination<'_>] = &[
    shell::Destination {
        label: "Overview",
        icon: icons::HOUSE,
    },
    shell::Destination {
        label: "Activities",
        icon: icons::ACTIVITY,
    },
    shell::Destination {
        label: "Courses",
        icon: icons::ROUTE,
    },
];

const DEVICE_DESTINATIONS: &[shell::Destination<'_>] = &[shell::Destination {
    label: "Mock Cycle-o-Matic 9000",
    icon: icons::BICYCLE,
}];

#[derive(Clone, Copy)]
struct SceneProps {
    navigation: shell::Navigation,
    active: usize,
    profile: bool,
    profile_open: bool,
    window_controls: bool,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Default)]
struct SelectorState {
    profile_open: bool,
    selected: usize,
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let props = SceneProps {
        navigation: if ctx.buttons("navigation", &["expanded", "rail"], 0) == 0 {
            shell::Navigation::Expanded
        } else {
            shell::Navigation::Rail
        },
        active: ctx.buttons(
            "active",
            &["overview", "activities", "courses", "devices"],
            1,
        ),
        profile: ctx.toggle("profile", true),
        profile_open: false,
        window_controls: ctx.toggle("window controls", true),
        width: ctx.slider("width", 960.0, 360.0, 1280.0, 1.0),
        height: ctx.slider("height", 600.0, 360.0, 800.0, 1.0),
    };
    show_shell(ctx, ui, globals, props, "playground");
}

#[scene]
fn expanded(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_shell(
        ctx,
        ui,
        globals,
        SceneProps {
            navigation: shell::Navigation::Expanded,
            active: 1,
            profile: true,
            profile_open: false,
            window_controls: true,
            width: 960.0,
            height: 600.0,
        },
        "expanded",
    );
}

#[scene]
fn rail(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_shell(
        ctx,
        ui,
        globals,
        SceneProps {
            navigation: shell::Navigation::Rail,
            active: 2,
            profile: true,
            profile_open: false,
            window_controls: true,
            width: 720.0,
            height: 520.0,
        },
        "rail",
    );
}

#[scene]
fn narrow(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_shell(
        ctx,
        ui,
        globals,
        SceneProps {
            navigation: shell::Navigation::Rail,
            active: 3,
            profile: true,
            profile_open: false,
            window_controls: true,
            width: 400.0,
            height: 640.0,
        },
        "narrow",
    );
}

#[scene]
fn narrow_navigation_open(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_shell(
        ctx,
        ui,
        globals,
        SceneProps {
            navigation: shell::Navigation::Expanded,
            active: 3,
            profile: true,
            profile_open: false,
            window_controls: false,
            width: 320.0,
            height: 720.0,
        },
        "narrow-navigation-open",
    );
}

#[scene]
fn profile_menu(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_shell(
        ctx,
        ui,
        globals,
        SceneProps {
            navigation: shell::Navigation::Expanded,
            active: 1,
            profile: true,
            profile_open: true,
            window_controls: false,
            width: 640.0,
            height: 480.0,
        },
        "profile-menu",
    );
}

fn accent_choice(ctx: &mut SceneCtx<'_>) -> garmin_color::Color {
    [
        swatch::cyan::G40,
        swatch::magenta::G50,
        swatch::green::G50,
        swatch::BLACK,
        swatch::WHITE,
        garmin_color::Color::from_rgba(0, 255, 0, 0),
    ][ctx.buttons(
        "accent",
        &["cyan", "pink", "green", "black", "white", "transparent"],
        0,
    )]
}

fn show_shell(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    props: SceneProps,
    state_name: &'static str,
) {
    let state_id = egui::Id::new(("shell-gallery-selectors", state_name));
    let mut state = ui
        .data(|data| data.get_temp::<SelectorState>(state_id))
        .unwrap_or(SelectorState {
            profile_open: props.profile_open,
            selected: 0,
        });
    let accent = accent_choice(ctx);
    let examples = ctx.toggle("accent examples", false);
    let profiles = [
        profile::ProfileProps {
            display_name: "Alex Rider",
            accent,
            avatar: None,
        },
        profile::ProfileProps {
            display_name: "Sam Runner",
            accent: swatch::magenta::G40,
            avatar: None,
        },
        profile::ProfileProps {
            display_name: "Taylor Cyclist",
            accent: swatch::green::G40,
            avatar: None,
        },
    ];
    let intl = globals.intl();
    let selector = profile::SelectorProps {
        intl: &intl,
        profiles: &profiles,
        selected: Some(state.selected),
        expanded: state.profile_open,
        backup_enabled: true,
    };
    let navigation_groups = [
        shell::NavigationGroup {
            label: None,
            destinations: PRIMARY_DESTINATIONS,
        },
        shell::NavigationGroup {
            label: Some("Attached devices"),
            destinations: DEVICE_DESTINATIONS,
        },
    ];
    let window_controls = shell::WindowControls {
        maximized: false,
        minimize_label: "Minimize window",
        maximize_label: "Maximize window",
        restore_label: "Restore window",
        close_label: "Close window",
    };
    stage!(ctx, ui, |ui| {
        ui.set_width(props.width);
        ui.set_height(props.height);
        let output = shell::show(
            ui,
            &shell::Props {
                product_name: "Garmin companion",
                navigation_groups: &navigation_groups,
                active: Some(props.active),
                navigation: props.navigation,
                profile_selector: props.profile.then_some(&selector),
                toggle_label: "Toggle navigation",
                profile_label: "Open profiles",
                window_controls: props.window_controls.then_some(&window_controls),
            },
            |ui| {
                let title = if props.active < PRIMARY_DESTINATIONS.len() {
                    PRIMARY_DESTINATIONS[props.active].label
                } else {
                    DEVICE_DESTINATIONS[props.active - PRIMARY_DESTINATIONS.len()].label
                };
                ui.heading(title);
                ui.add_space(16.0);
                if examples {
                    accent_examples(ui);
                } else {
                    ui.label("Page content is supplied by the host application.");
                }
            },
        );
        match output.action {
            Some(shell::Action::Profile(profile::Action::Toggle)) => {
                state.profile_open = !state.profile_open;
            }
            Some(shell::Action::Profile(profile::Action::Select(index))) => {
                state.selected = index;
                state.profile_open = false;
            }
            Some(shell::Action::Profile(_)) => state.profile_open = false,
            _ => {}
        }
    });
    ui.data_mut(|data| data.insert_temp(state_id, state));
}

fn accent_examples(ui: &mut Ui) {
    use garmin_ui::{Size, button, path, progress, radio};
    let id = ui.id().with("accent-examples");
    let (mut metric, mut checked, mut first) = ui
        .data(|data| data.get_temp::<(bool, bool, bool)>(id))
        .unwrap_or((true, true, true));
    if let Some(value) = radio::show(
        ui,
        metric,
        &[
            radio::Choice::new("Metric", true, "accent.metric"),
            radio::Choice::new("Imperial", false, "accent.imperial"),
        ],
        radio::Props {
            label: "Units",
            helper: None,
            enabled: true,
        },
    ) {
        metric = value;
    }
    garmin_ui::theme::selected_control(ui, checked, |ui| {
        ui.checkbox(&mut checked, "Selected checkbox")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    });
    ui.add_space(8.0);
    if let Some(value) = button::group(
        ui,
        first,
        &[
            button::GroupChoice::new("Selected", icons::SUN, true),
            button::GroupChoice::new("Other", icons::MOON, false),
        ],
        button::GroupProps {
            size: Size::Medium,
            width: button::Width::Fit,
            enabled: true,
            style: button::GroupStyle::Subtle,
        },
    ) {
        first = value;
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (label, kind) in [
            ("Primary action", button::Kind::Primary),
            ("Destructive action", button::Kind::Danger),
        ] {
            button::Props {
                label,
                icon: None,
                kind,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: true,
            }
            .show(ui);
        }
    });
    ui.add_space(16.0);
    progress::show(
        ui,
        &progress::Props {
            label: "Importing activities",
            detail: Some("3 of 8 files"),
            value: progress::Value::Determinate {
                completed: 3,
                total: 8,
            },
            height: None,
        },
    );
    ui.add_space(16.0);
    path::preview(
        ui,
        &path::Props {
            segments: &[path::Segment {
                points: &[
                    path::Point {
                        latitude: 0.0,
                        longitude: 0.0,
                    },
                    path::Point {
                        latitude: 0.8,
                        longitude: 0.4,
                    },
                    path::Point {
                        latitude: 0.2,
                        longitude: 1.0,
                    },
                    path::Point {
                        latitude: 0.9,
                        longitude: 1.4,
                    },
                ],
            }],
            empty: "No route",
            height: Some(120.0),
        },
    );
    ui.data_mut(|data| data.insert_temp(id, (metric, checked, first)));
}
