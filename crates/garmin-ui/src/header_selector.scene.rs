use gallery::prelude::*;
use garmin_color::swatch;
use garmin_ui::{profile, shell};

scene_meta! { title: "Components / Navigation / Header selector" }

#[scene(default)]
fn profile_menu(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let id = egui::Id::new("profile-selector-gallery-expanded");
    let mut expanded = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(true);
    let intl = globals.intl();
    let profiles = [profile::ProfileProps {
        display_name: "Alex Rider",
        accent: swatch::cyan::G40,
        avatar: None,
    }];

    stage!(ctx, ui, (640, 160), |ui| {
        let props = profile::SelectorProps {
            intl: &intl,
            profiles: &profiles,
            selected: Some(0),
            expanded,
        };
        let output = shell::show(
            ui,
            &shell::Props {
                product_name: "",
                navigation_groups: &[],
                active: None,
                navigation: shell::Navigation::Rail,
                profile_selector: Some(&props),
                toggle_label: "",
                profile_label: "Open profile menu",
                window_controls: None,
            },
            |_| {},
        );
        match output.action {
            Some(shell::Action::Profile(profile::Action::Toggle)) => expanded = !expanded,
            Some(shell::Action::Profile(_)) => expanded = false,
            _ => {}
        }
    });
    ui.data_mut(|data| data.insert_temp(id, expanded));
}
