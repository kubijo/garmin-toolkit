//! Initials stay readable for arbitrary profile accents in either theme.

use gallery::egui;
use garmin_color::Color;
use garmin_ui::profile::AvatarProps;

#[test]
fn initials_contrast_survives_accent_extremes_and_transparency() {
    for theme in [egui::ThemePreference::Dark, egui::ThemePreference::Light] {
        let context = egui::Context::default();
        garmin_ui::install(&context);
        context.set_theme(theme);
        for red in [0, 64, 128, 192, 255] {
            for green in [0, 64, 128, 192, 255] {
                for blue in [0, 64, 128, 192, 255] {
                    for alpha in [0, 128, 255] {
                        let accent = Color::from_rgba(red, green, blue, alpha);
                        let output = context.run_ui(egui::RawInput::default(), |ui| {
                            AvatarProps {
                                display_name: "Alex Rider",
                                accent,
                                image: None,
                                size: 48.0,
                            }
                            .show(ui);
                        });
                        let background = output
                            .shapes
                            .iter()
                            .find_map(|shape| {
                                if let egui::Shape::Circle(circle) = &shape.shape
                                    && (circle.radius - 22.0).abs() < f32::EPSILON
                                {
                                    Some(circle.fill)
                                } else {
                                    None
                                }
                            })
                            .expect("initials background");
                        let foreground = output
                            .shapes
                            .iter()
                            .find_map(|shape| {
                                if let egui::Shape::Text(text) = &shape.shape {
                                    Some(text.galley.job.sections[0].format.color)
                                } else {
                                    None
                                }
                            })
                            .expect("initials text");
                        assert_eq!(background.a(), 255);
                        let contrast = color(background).contrast_ratio(color(foreground));
                        assert!(contrast >= 4.5, "{theme:?}, {accent}: {contrast}");
                        output.drop_without_applying_deltas();
                    }
                }
            }
        }
    }
}

fn color(value: egui::Color32) -> Color {
    let [red, green, blue, alpha] = value.to_srgba_unmultiplied();
    Color::from_rgba(red, green, blue, alpha)
}
