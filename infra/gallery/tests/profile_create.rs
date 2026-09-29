//! Profile creation through the same semantic targets exposed by the Control API.

use egui_kittest::{
    Harness,
    kittest::{By, Queryable as _},
};
use garmin_i18n::{Language, Translations};
use garmin_ui::profile;

#[derive(Default)]
struct Model {
    form: Option<profile::CreateState>,
    created: Option<String>,
}

#[test]
fn profile_creation_and_cancellation_use_locale_independent_targets() {
    for language in [Language::English, Language::Czech] {
        let intl = Translations::bundled()
            .expect("catalog")
            .formatter(language)
            .expect("supported language");
        let mut installed = false;
        let mut view = Harness::builder().with_size([960.0, 720.0]).build_ui_state(
            move |ui, model: &mut Model| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                if let Some(form) = &mut model.form {
                    match profile::create_dialog(ui, &intl, form) {
                        Some(profile::CreateAction::Submit(name)) => {
                            model.created = Some(name.as_str().to_owned());
                            model.form = None;
                        }
                        Some(profile::CreateAction::Cancel) => model.form = None,
                        None => {}
                    }
                } else if matches!(
                    profile::chooser(
                        ui,
                        &profile::ChooserProps {
                            intl: &intl,
                            profiles: &[]
                        }
                    ),
                    Some(profile::Action::Create)
                ) {
                    model.form = Some(profile::CreateState::default());
                }
            },
            Model::default(),
        );
        view.run();
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.create")))
            .click();
        view.run();
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.create.name")))
            .click();
        view.run();
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.create.name")))
            .type_text("Restore check");
        view.run();
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.create.submit")))
            .click();
        view.run();
        assert_eq!(view.state().created.as_deref(), Some("Restore check"));
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.create")))
            .click();
        view.run();
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.create.cancel")))
            .click();
        view.run();
        assert!(view.state().form.is_none());
    }
}
