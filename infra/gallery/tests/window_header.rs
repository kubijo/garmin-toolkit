//! Shared native title-bar geometry and actions, independent of the window manager.
use egui_kittest::{Harness, kittest::Queryable as _};
use garmin_ui::shell;
use garmin_ui::shell::{WindowAction, WindowControls};

#[test]
fn open_window_controls_follow_language_changes() {
    use garmin_i18n::{Language, Translations};
    let translations = Translations::bundled().expect("bundled translations");
    let mut installed = false;
    let mut harness = Harness::new_ui_state(
        move |ui, language: &mut Language| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            let intl = translations.formatter(*language).expect("supported locale");
            let labels = shell::WindowLabels::new(&intl);
            let controls = labels.props(ui.ctx());
            let _ = shell::window_header(ui, "Files", &controls);
        },
        Language::English,
    );
    harness.run();
    let _ = harness.get_by_label("Minimize window");
    *harness.state_mut() = Language::Czech;
    harness.run();
    for label in ["Minimalizovat okno", "Maximalizovat okno", "Zavřít okno"] {
        let _ = harness.get_by_label(label);
    }
}

#[test]
fn child_window_controls_stay_in_the_header_and_emit_their_own_actions() {
    for width in [380.0, 680.0] {
        for (maximized, label, expected) in [
            (false, "Minimize window", WindowAction::Minimize),
            (false, "Maximize window", WindowAction::ToggleMaximize),
            (true, "Restore window", WindowAction::ToggleMaximize),
            (false, "Close window", WindowAction::Close),
        ] {
            let mut installed = false;
            let mut harness = Harness::builder().with_size([width, 240.0]).build_ui_state(
                move |ui, action: &mut Option<WindowAction>| {
                    if !installed {
                        garmin_ui::install(ui.ctx());
                        installed = true;
                        ui.ctx().request_repaint();
                        return;
                    }
                    garmin_ui::window::surface(ui, |ui| {
                        if let Some(requested) = shell::window_header(
                            ui,
                            "Developer tools",
                            &WindowControls {
                                maximized,
                                minimize_label: "Minimize window",
                                maximize_label: "Maximize window",
                                restore_label: "Restore window",
                                close_label: "Close window",
                            },
                        ) {
                            *action = Some(requested);
                        }
                        ui.label("Window contents");
                    });
                },
                None,
            );
            harness.run();
            let button = harness.get_by_label(label).rect();
            let body = harness.get_by_label("Window contents").rect();
            assert!(button.left() >= 13.0 && button.right() <= width - 13.0);
            assert!(button.bottom() <= body.top());
            assert!((button.width() - 32.0).abs() < 0.1);
            assert!((button.height() - 32.0).abs() < 0.1);
            harness.get_by_label(label).click();
            harness.run();
            assert_eq!(*harness.state(), Some(expected));
        }
    }
}
