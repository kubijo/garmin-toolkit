use gallery::prelude::*;
use garmin_color::swatch;
use garmin_i18n::{Intl, Language, Translations};
use garmin_ui::{icons, profile, shell};
use std::sync::OnceLock;

scene_meta! { title: "Desktop / Shell" }

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
    label: "Garmin Edge 1050",
    icon: icons::BICYCLE,
}];

#[derive(Clone, Copy)]
struct SceneProps {
    navigation: shell::Navigation,
    active: usize,
    profile: bool,
    window_controls: bool,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Default)]
struct SelectorState {
    profile_open: bool,
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
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
        window_controls: ctx.toggle("window controls", true),
        width: ctx.slider("width", 960.0, 360.0, 1280.0, 1.0),
        height: ctx.slider("height", 600.0, 360.0, 800.0, 1.0),
    };
    show_shell(ctx, ui, props, "playground");
}

#[scene]
fn expanded(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    show_shell(
        ctx,
        ui,
        SceneProps {
            navigation: shell::Navigation::Expanded,
            active: 1,
            profile: true,
            window_controls: true,
            width: 960.0,
            height: 600.0,
        },
        "expanded",
    );
}

#[scene]
fn rail(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    show_shell(
        ctx,
        ui,
        SceneProps {
            navigation: shell::Navigation::Rail,
            active: 2,
            profile: true,
            window_controls: true,
            width: 720.0,
            height: 520.0,
        },
        "rail",
    );
}

#[scene]
fn narrow(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    show_shell(
        ctx,
        ui,
        SceneProps {
            navigation: shell::Navigation::Rail,
            active: 3,
            profile: true,
            window_controls: true,
            width: 400.0,
            height: 640.0,
        },
        "narrow",
    );
}

#[scene]
fn light(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply_palette(ui.style_mut(), &garmin_color::theme::GRAY_10);
    show_shell(
        ctx,
        ui,
        SceneProps {
            navigation: shell::Navigation::Expanded,
            active: 1,
            profile: true,
            window_controls: true,
            width: 960.0,
            height: 600.0,
        },
        "light",
    );
}

fn show_shell(ctx: &mut SceneCtx<'_>, ui: &mut Ui, props: SceneProps, state_name: &'static str) {
    let state_id = egui::Id::new(("shell-gallery-selectors", state_name));
    let mut state = ui
        .data(|data| data.get_temp::<SelectorState>(state_id))
        .unwrap_or_default();
    let profiles = [
        profile::ProfileProps {
            display_name: "Alex Rider",
            accent: swatch::cyan::G40,
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
    let intl = formatter();
    let selector = profile::SelectorProps {
        intl: &intl,
        profiles: &profiles,
        selected: Some(0),
        expanded: state.profile_open,
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
                ui.label("Page content is supplied by the host application.");
            },
        );
        match output.action {
            Some(shell::Action::Profile(profile::Action::Toggle)) => {
                state.profile_open = !state.profile_open;
            }
            Some(shell::Action::Profile(_)) => state.profile_open = false,
            _ => {}
        }
    });
    ui.data_mut(|data| data.insert_temp(state_id, state));
}

fn formatter() -> Intl {
    static TRANSLATIONS: OnceLock<Translations> = OnceLock::new();
    TRANSLATIONS
        .get_or_init(|| {
            Translations::bundled().expect("embedded catalogs are validated during the build")
        })
        .formatter(Language::English)
        .expect("the gallery requests a bundled language")
}
