//! Profile styling follows the active shell without leaking into other profiles or semantic colors.

use gallery::egui;
use garmin_color::{Color, swatch, theme as colors};
use garmin_ui::{profile, radio, shell, theme};

#[test]
fn arbitrary_accents_do_not_change_segment_fills_or_action_buttons() {
    use garmin_ui::{Size, button, icons};

    for preference in [egui::ThemePreference::Dark, egui::ThemePreference::Light] {
        let context = egui::Context::default();
        garmin_ui::install(&context);
        context.set_theme(preference);
        let mut baseline = None;
        for accent in [
            swatch::ACTION,
            swatch::BLACK,
            swatch::WHITE,
            swatch::magenta::G50,
            Color::from_rgba(0, 255, 0, 0),
        ] {
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                theme::with_profile_accent(ui, accent, |ui| {
                    let _ = button::group(
                        ui,
                        true,
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
                    );
                    button::Props {
                        label: "Primary",
                        icon: None,
                        kind: button::Kind::Primary,
                        size: Size::Medium,
                        width: button::Width::Fit,
                        enabled: true,
                    }
                    .show(ui);
                });
            });
            let fills = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Rect(rect) = &shape.shape
                        && rect.rect.height() > 4.0
                    {
                        Some(rect.fill)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert!(
                fills.len() >= 3,
                "both options and the primary action must be rendered"
            );
            if let Some(baseline) = &baseline {
                assert_eq!(
                    &fills, baseline,
                    "profile accent must not become a button fill"
                );
            } else {
                baseline = Some(fills);
            }
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn shell_accent_follows_profile_switches_and_does_not_leak() {
    let intl = garmin_i18n::Translations::bundled()
        .expect("bundled translations")
        .formatter(garmin_i18n::Language::English)
        .expect("English formatter");
    let profiles = [
        profile::ProfileProps {
            display_name: "Pink",
            accent: swatch::magenta::G50,
            avatar: None,
        },
        profile::ProfileProps {
            display_name: "Green",
            accent: swatch::green::G50,
            avatar: None,
        },
    ];
    for preference in [egui::ThemePreference::Dark, egui::ThemePreference::Light] {
        let context = egui::Context::default();
        garmin_ui::install(&context);
        context.set_theme(preference);
        for selected in [Some(0), Some(1), None, Some(0)] {
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    let before = theme::selection_accent(ui);
                    let palette = *theme::palette(ui);
                    let selector = profile::SelectorProps {
                        intl: &intl,
                        profiles: &profiles,
                        selected,
                        expanded: false,
                    };
                    let _ = shell::show(
                        ui,
                        &shell::Props {
                            product_name: "Test",
                            navigation_groups: &[],
                            active: None,
                            navigation: shell::Navigation::Expanded,
                            profile_selector: Some(&selector),
                            toggle_label: "Navigation",
                            profile_label: "Profile",
                            window_controls: None,
                        },
                        |ui| {
                            let accent = selected.map_or_else(
                                || palette.interaction().interactive(),
                                |index| profiles[index].accent,
                            );
                            assert_eq!(
                                theme::selection_accent(ui),
                                accent.contrasting_marker(
                                    palette.surfaces().layer(colors::Level::One)
                                )
                            );
                            assert_eq!(
                                *theme::palette(ui),
                                palette,
                                "semantic roles must stay unchanged"
                            );
                            theme::with_profile_accent(ui, swatch::WHITE, |ui| {
                                assert_eq!(
                                    theme::selection_accent(ui),
                                    swatch::WHITE.contrasting_marker(
                                        palette.surfaces().layer(colors::Level::One)
                                    )
                                );
                            });
                            assert_eq!(
                                theme::selection_accent(ui),
                                accent.contrasting_marker(
                                    palette.surfaces().layer(colors::Level::One)
                                )
                            );
                        },
                    );
                    assert_eq!(theme::selection_accent(ui), before);
                })
                .drop_without_applying_deltas();
        }
    }
}

#[test]
fn profile_markers_are_opaque_and_visible_on_each_surface() {
    for preference in [egui::ThemePreference::Dark, egui::ThemePreference::Light] {
        let context = egui::Context::default();
        garmin_ui::install(&context);
        context.set_theme(preference);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                for accent in [
                    swatch::BLACK,
                    swatch::WHITE,
                    swatch::magenta::G50,
                    Color::from_rgba(0, 255, 0, 0),
                ] {
                    theme::with_profile_accent(ui, accent, |ui| {
                        let palette = theme::palette(ui);
                        for level in [colors::Level::One, colors::Level::Two, colors::Level::Three]
                        {
                            for surface in [
                                palette.surfaces().layer(level),
                                palette.surfaces().layer_hover(level),
                                palette.surfaces().field(level),
                            ] {
                                let resolved = theme::selection_accent_on(ui, surface);
                                assert_eq!(resolved.as_rgba()[3], 255);
                                assert!(resolved.contrast_ratio(surface) >= 3.0);
                            }
                        }
                    });
                }
            })
            .drop_without_applying_deltas();
    }
}

#[test]
fn selected_radio_marks_are_accented_but_labels_and_disabled_marks_are_not() {
    let context = egui::Context::default();
    garmin_ui::install(&context);
    for preference in [egui::ThemePreference::Dark, egui::ThemePreference::Light] {
        context.set_theme(preference);
        for enabled in [true, false] {
            let mut expected = egui::Color32::TRANSPARENT;
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                theme::with_profile_accent(ui, swatch::magenta::G50, |ui| {
                    let palette = theme::palette(ui);
                    let [r, g, b, _] = theme::selection_accent_on(
                        ui,
                        palette.surfaces().field(colors::Level::One),
                    )
                    .as_rgba();
                    expected = egui::Color32::from_rgb(r, g, b);
                    let _ = radio::show(
                        ui,
                        true,
                        &[radio::Choice::new("Selected", true, "selected")],
                        radio::Props {
                            label: "Units",
                            helper: None,
                            enabled,
                        },
                    );
                });
            });
            let mut dot_is_accented = false;
            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::Circle(circle) if circle.radius < 4.0 => {
                        dot_is_accented |= circle.fill == expected;
                    }
                    egui::Shape::Text(text) => {
                        assert_ne!(text.galley.job.sections[0].format.color, expected);
                    }
                    _ => {}
                }
            }
            assert_eq!(dot_is_accented, enabled);
            output.drop_without_applying_deltas();
        }
    }
}
