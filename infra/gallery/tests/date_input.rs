//! Date entry, calendar navigation, and controlled-value behavior.
use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use gallery::egui;

fn harness(enabled: bool) -> Harness<'static, String> {
    let intl = garmin_i18n::Translations::bundled()
        .expect("translations")
        .formatter(garmin_i18n::Language::English)
        .expect("formatter");
    let mut installed = false;
    Harness::builder().with_size([400.0, 480.0]).build_ui_state(
        move |ui, value| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
            }
            ui.set_width(320.0);
            let response = garmin_ui::date_input::show(
                ui,
                egui::Id::new("date-test"),
                value,
                &intl,
                "From date",
                enabled,
                jiff::civil::date(2026, 11, 13),
            );
            garmin_ui::semantics::target(ui, &response, "date-test.field");
        },
        String::new(),
    )
}

#[test]
fn selecting_a_day_updates_the_value_and_closes_the_popup() {
    let mut view = harness(true);
    view.run();
    assert_eq!(view.state(), "");
    view.get(By::new().predicate(|node| node.author_id() == Some("date-test.field")))
        .click();
    view.run();
    view.get_by_label("Previous month").click();
    view.run();
    let day = || By::new().predicate(|node| node.author_id() == Some("date-input.day.2026-10-04"));
    view.get(day()).click();
    view.run();
    assert_eq!(view.state(), "2026-10-04");
    assert!(view.query(day()).is_none());
    view.get_by_label("Choose date for From date").click();
    view.run();
    assert!(
        view.query(day()).is_some(),
        "reopening follows the chosen month"
    );
    view.key_press(egui::Key::Escape);
    view.run();
    assert_eq!(view.state(), "2026-10-04");
}

#[test]
fn disabled_date_input_cannot_open_a_calendar() {
    let mut view = harness(false);
    view.run();
    assert!(
        view.get_by_label("Choose date for From date")
            .accesskit_node()
            .is_disabled()
    );
    assert!(view.query_by_label("Previous month").is_none());
}

#[test]
fn day_cells_have_constant_size_and_four_pixel_gaps() {
    let mut view = harness(true);
    view.run();
    view.get_by_label("Choose date for From date").click();
    view.run();
    let rect = |day| {
        let target = format!("date-input.day.2026-11-{day:02}");
        view.get(By::new().predicate(move |node| node.author_id() == Some(target.as_str())))
            .rect()
    };
    for day in 1..=30 {
        assert_eq!(rect(day).size(), egui::vec2(40.0, 32.0));
    }
    assert!((rect(2).left() - rect(1).right() - 4.0).abs() < 0.01);
    assert!((rect(8).top() - rect(1).bottom() - 4.0).abs() < 0.01);
}

#[test]
#[ignore = "renders the calendar popup in both themes"]
fn capture_date_picker() -> Result<(), Box<dyn std::error::Error>> {
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/gallery/date-picker");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("dark", egui::ThemePreference::Dark),
        ("light", egui::ThemePreference::Light),
    ] {
        let mut view = harness(true);
        view.ctx.set_theme(theme);
        view.run();
        view.get_by_label("Choose date for From date").click();
        view.run();
        view.render()?.save(out.join(format!("{name}.png")))?;
        view.get(By::new().predicate(|node| node.author_id() == Some("date-input.day.2026-11-13")))
            .click();
        view.run();
        view.get_by_label("Choose date for From date").click();
        view.run();
        view.render()?
            .save(out.join(format!("{name}-selected.png")))?;
    }
    Ok(())
}
