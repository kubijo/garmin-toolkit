use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use gallery::egui;

#[test]
fn application_buttons_have_a_pointer_cursor_in_both_themes() {
    for dark in [true, false] {
        let mut installed = false;
        let mut harness = Harness::builder().build_ui(move |ui| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            garmin_ui::theme::apply_palette(
                ui.style_mut(),
                if dark {
                    &garmin_color::theme::GRAY_100
                } else {
                    &garmin_color::theme::GRAY_10
                },
            );
            let _ = ui.button("Ordinary button");
            ui.add_enabled(false, egui::Button::new("Disabled button"));
        });
        harness.run();
        harness.get_by_label("Ordinary button").hover();
        harness.run();
        assert_eq!(
            harness.output().platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
        harness.get_by_label("Disabled button").hover();
        harness.run();
        assert_ne!(
            harness.output().platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
    }
}

#[test]
fn disclosure_preserves_pointer_and_keyboard_access_to_its_content() {
    let mut installed = false;
    let mut harness = Harness::builder()
        .with_size([320.0, 320.0])
        .build_ui(move |ui| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            garmin_ui::accordion::show(
                ui,
                &garmin_ui::accordion::Props {
                    id: "details",
                    label: "Details",
                    default_open: false,
                    inline_padding: 16,
                },
                |ui| {
                    ui.label("Retained plan evidence");
                },
            );
        });
    harness.run();
    assert!(harness.query_by_label("Retained plan evidence").is_none());
    let header = harness.get(By::new().predicate(|node| node.author_id() == Some("details")));
    assert_eq!(header.accesskit_node().data().is_expanded(), Some(false));
    let rest = header
        .accesskit_node()
        .data()
        .bounds()
        .expect("header bounds");
    header.hover();
    harness.run();
    let hover = harness
        .get(By::new().predicate(|node| node.author_id() == Some("details")))
        .accesskit_node()
        .data()
        .bounds()
        .expect("hovered header bounds");
    assert_eq!(rest, hover, "hover must not move disclosure content");
    assert_eq!(
        harness.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    harness
        .get(By::new().predicate(|node| node.author_id() == Some("details")))
        .click();
    harness.run();
    assert!(harness.query_by_label("Retained plan evidence").is_some());
    harness
        .get(By::new().predicate(|node| node.author_id() == Some("details")))
        .focus();
    harness.run();
    harness.key_press(egui::Key::Space);
    harness.run();
    assert!(harness.query_by_label("Retained plan evidence").is_none());
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(harness.query_by_label("Retained plan evidence").is_some());
}
