use gallery::prelude::*;
use garmin_color::swatch;
use garmin_i18n::{Intl, Language, Translations};
use garmin_ui::{profile, shell};
use std::sync::OnceLock;

scene_meta! { title: "Components / Navigation / Header selector" }

#[scene(default)]
fn profile_menu(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let id = egui::Id::new("profile-selector-gallery-expanded");
    let mut expanded = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(true);
    let intl = formatter();
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

fn formatter() -> Intl {
    static TRANSLATIONS: OnceLock<Translations> = OnceLock::new();
    TRANSLATIONS
        .get_or_init(|| {
            Translations::bundled().expect("embedded catalogs are validated during the build")
        })
        .formatter(Language::English)
        .expect("the gallery requests a bundled language")
}
