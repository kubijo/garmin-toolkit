//! Profile controls preserve independent preferences and scope draft colors by identity.
use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use gallery::egui;
use garmin_color::{Color, swatch};
use garmin_model::identity::ProfilePreferences;
use garmin_ui::profile_settings::{self, Action};

struct Model {
    profile: u8,
    accent: Option<Color>,
    preferences: ProfilePreferences,
    disabled: bool,
}

fn harness() -> Harness<'static, Model> {
    harness_for(garmin_i18n::Language::English, [760.0, 1000.0])
}

fn harness_for(language: garmin_i18n::Language, size: [f32; 2]) -> Harness<'static, Model> {
    let intl = garmin_i18n::Translations::bundled()
        .expect("bundled translations")
        .formatter(language)
        .expect("locale formatter");
    let mut installed = false;
    Harness::builder().with_size(size).build_ui_state(
        move |ui, state: &mut Model| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            if let Some(action) = profile_settings::show(
                ui,
                &profile_settings::Props {
                    id: egui::Id::new(state.profile),
                    intl: &intl,
                    preferences: state.preferences,
                    accent: state.accent,
                    profile: garmin_ui::profile::ProfileProps {
                        display_name: "Same name",
                        accent: state.accent.unwrap_or(swatch::ACTION),
                        avatar: None,
                    },
                    picture_enabled: false,
                    disabled: state.disabled,
                },
            ) {
                match action {
                    Action::UpdateAccent(accent) => state.accent = accent,
                    Action::UpdatePreferences(preferences) => state.preferences = preferences,
                    Action::ChoosePicture => {}
                }
            }
        },
        Model {
            profile: 0,
            accent: None,
            disabled: false,
            preferences: ProfilePreferences::default()
                .with_show_hidden_files(true)
                .with_inline_file_windows(true),
        },
    )
}

#[test]
fn scrolling_from_the_empty_side_moves_the_heading_and_reaches_preferences() {
    let mut view = harness_for(garmin_i18n::Language::English, [960.0, 480.0]);
    view.run();
    assert!(view.get_by_label("Profile settings").rect().top() > 0.0);
    view.hover_at(egui::pos2(900.0, 240.0));
    view.event(egui::Event::MouseWheel {
        phase: egui::TouchPhase::Move,
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -600.0),
        modifiers: egui::Modifiers::NONE,
    });
    view.run();
    assert!(
        view.query_by_label("Profile settings")
            .is_none_or(|node| node.rect().bottom() <= 0.0)
    );
    let field = view
        .get(By::new().predicate(|node| node.author_id() == Some("profile.file-windows.inline")));
    assert!(field.rect().top() >= 0.0 && field.rect().bottom() <= 480.0);
}

#[test]
fn narrow_czech_settings_keep_theme_choices_visible_and_clickable() {
    let mut view = harness_for(garmin_i18n::Language::Czech, [320.0, 1000.0]);
    view.run();
    for target in [
        "profile.units.metric",
        "profile.units.imperial",
        "profile.language.english",
        "profile.language.czech",
        "profile.file-windows.separate",
        "profile.file-windows.inline",
    ] {
        let bounds = view
            .get(By::new().predicate(|node| node.author_id() == Some(target)))
            .rect();
        assert!(
            bounds.left() >= 0.0 && bounds.right() <= 320.0,
            "{target}: {bounds:?}"
        );
    }
    for label in ["Automaticky", "Tmavý", "Světlý"] {
        let bounds = view.get_by_label(label).rect();
        assert!(
            bounds.left() >= 0.0 && bounds.right() <= 320.0,
            "{label}: {bounds:?}"
        );
    }
    assert!(
        view.get_by_label("Světlý").rect().top()
            >= view.get_by_label("Automaticky").rect().bottom(),
        "wrap whole buttons instead of splitting their labels"
    );
    view.get_by_label("Světlý").click();
    view.run();
    assert_eq!(
        view.state().preferences.theme(),
        garmin_model::identity::ThemePreference::Light
    );
    assert!(view.state().preferences.show_hidden_files());
    assert!(view.state().preferences.inline_file_windows());
}

#[test]
fn custom_accent_requires_valid_input_and_apply_and_can_reset() {
    let mut view = harness();
    view.run();
    let default_ring = avatar_ring(&view);
    open_picker(&mut view);
    let hex = || By::new().role(egui::accesskit::Role::TextInput);
    let apply = || By::new().predicate(|node| node.author_id() == Some("profile.accent.apply"));
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#FF");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    assert!(view.get(apply()).accesskit_node().is_disabled());
    assert_eq!(view.state().accent, None);
    open_picker(&mut view);
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#C855A8");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    assert_eq!(view.state().accent, None);
    assert_eq!(avatar_ring(&view), egui::Color32::from_rgb(200, 85, 168));
    let apply_center = view.get(apply()).rect().center();
    view.hover_at(apply_center);
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    view.get(apply()).click();
    view.run();
    assert_eq!(view.state().accent, Some(Color::from_rgb(200, 85, 168)));
    assert_eq!(avatar_ring(&view), egui::Color32::from_rgb(200, 85, 168));
    assert!(view.state().preferences.inline_file_windows());
    assert!(view.state().preferences.show_hidden_files());
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.reset")))
        .click();
    view.run();
    assert_eq!(view.state().accent, None);
    assert_eq!(avatar_ring(&view), default_ring);
}

fn avatar_ring(view: &Harness<'_, Model>) -> egui::Color32 {
    view.output()
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::epaint::Shape::Circle(circle) = &shape.shape
                && (circle.radius - 32.0).abs() < f32::EPSILON
            {
                Some(circle.fill)
            } else {
                None
            }
        })
        .expect("profile avatar's outer circle")
}

#[test]
fn window_and_theme_changes_preserve_other_profile_preferences() {
    let mut view = harness();
    view.run();
    view.get_by_label("Separate window").click();
    view.run();
    assert!(!view.state().preferences.inline_file_windows());
    view.get_by_label("Light").click();
    view.run();
    assert!(view.state().preferences.show_hidden_files());
    assert!(!view.state().preferences.inline_file_windows());
}

#[test]
fn binary_preferences_are_visible_radios_and_preserve_other_settings() {
    let mut view = harness();
    view.run();
    for label in [
        "Metric",
        "Imperial",
        "English",
        "Čeština",
        "Separate window",
        "Inside the app",
    ] {
        assert_eq!(
            view.get_by_label(label).accesskit_node().role(),
            egui::accesskit::Role::RadioButton
        );
    }
    view.get_by_label("Imperial").click();
    view.run();
    assert_eq!(
        view.state().preferences.unit_system(),
        garmin_model::identity::UnitSystem::Imperial
    );
    view.get_by_label("Čeština").click();
    view.run();
    assert_eq!(
        view.state().preferences.language(),
        garmin_model::identity::LanguagePreference::Czech
    );
    assert_eq!(
        view.state().preferences.unit_system(),
        garmin_model::identity::UnitSystem::Imperial
    );
    assert!(view.state().preferences.show_hidden_files());
    assert!(view.state().preferences.inline_file_windows());
    view.get_by_label("Separate window").focus();
    view.key_press(egui::Key::Space);
    view.run();
    assert!(!view.state().preferences.inline_file_windows());
}

#[test]
fn radio_controls_have_correct_cursors_and_cannot_change_when_disabled() {
    let mut view = harness();
    view.run();
    let center = view.get_by_label("Imperial").rect().center();
    view.hover_at(center);
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    view.state_mut().disabled = true;
    view.run();
    assert!(view.get_by_label("Imperial").accesskit_node().is_disabled());
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::NotAllowed
    );
    let initial = view.state().preferences;
    view.get_by_label("Imperial").click();
    view.run();
    assert_eq!(view.state().preferences, initial);
}

#[test]
fn color_drafts_do_not_cross_profiles_with_the_same_name() {
    let mut view = harness();
    view.run();
    let default_ring = avatar_ring(&view);
    open_picker(&mut view);
    let hex = || By::new().role(egui::accesskit::Role::TextInput);
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#C855A8");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    view.state_mut().profile = 1;
    view.run();
    assert_eq!(avatar_ring(&view), default_ring);
    assert!(
        view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.apply")))
            .accesskit_node()
            .is_disabled()
    );
    open_picker(&mut view);
    assert_ne!(view.get(hex()).value().as_deref(), Some("#C855A8"));
}

fn open_picker(view: &mut Harness<'_, Model>) {
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.picker")))
        .click();
    view.run();
}

#[test]
fn accent_controls_show_pointer_when_enabled_and_not_allowed_when_disabled() {
    let mut view = harness();
    view.run();
    for (target, expected) in [
        ("profile.accent.picker", egui::CursorIcon::PointingHand),
        ("profile.accent.apply", egui::CursorIcon::NotAllowed),
        ("profile.accent.reset", egui::CursorIcon::NotAllowed),
    ] {
        let center = view
            .get(By::new().predicate(|node| node.author_id() == Some(target)))
            .rect()
            .center();
        view.hover_at(center);
        view.run();
        assert_eq!(
            view.output().platform_output.cursor_icon,
            expected,
            "{target}"
        );
    }
    view.state_mut().disabled = true;
    view.run();
    let center = view
        .get(By::new().predicate(|node| node.author_id() == Some("profile.accent.picker")))
        .rect()
        .center();
    view.hover_at(center);
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        egui::CursorIcon::NotAllowed
    );
}

#[test]
fn picker_keeps_alpha_when_applying() {
    let mut view = harness();
    view.run();
    let style = view.ctx.global_style();
    open_picker(&mut view);
    assert_eq!(view.ctx.global_style(), style);
    let hex = || By::new().role(egui::accesskit::Role::TextInput);
    view.get(hex()).click();
    view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    view.get(hex()).type_text("#00FF0080");
    view.key_press(egui::Key::Enter);
    view.key_press(egui::Key::Escape);
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("profile.accent.apply")))
        .click();
    view.run();
    assert_eq!(view.state().accent, Some(Color::from_rgba(0, 255, 0, 128)));
}

#[test]
#[ignore = "renders the open accent picker with a headless GPU"]
fn capture_accent_picker() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/profile-accent");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("dark", egui::ThemePreference::Dark),
        ("light", egui::ThemePreference::Light),
    ] {
        let mut view = harness();
        view.run();
        view.ctx.set_theme(theme);
        view.run();
        open_picker(&mut view);
        view.render()?.save(out.join(format!("{name}.png")))?;
        let hex = || By::new().role(egui::accesskit::Role::TextInput);
        view.get(hex()).click();
        view.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        view.get(hex()).type_text("#C855A8");
        view.key_press(egui::Key::Enter);
        view.key_press(egui::Key::Escape);
        view.run();
        view.render()?
            .save(out.join(format!("{name}-preview.png")))?;
    }
    Ok(())
}
