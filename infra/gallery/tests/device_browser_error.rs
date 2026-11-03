//! Catalogue failures disappear on recovery without dismissing unrelated notifications.
use egui_kittest::{Harness, kittest::Queryable as _};
use garmin_i18n::{Language, Translations};
use garmin_ui::{device_browser, notification};

#[test]
fn catalogue_error_recovers_without_clearing_an_unrelated_notice() {
    let intl = Translations::bundled()
        .expect("bundled translations are valid")
        .formatter(Language::English)
        .expect("English translations are available");
    let mut installed = false;
    let mut view = Harness::new_ui_state(
        move |ui, error: &mut Option<String>| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            notification::show(
                ui,
                &notification::Props {
                    kind: notification::Kind::Error,
                    title: "An unrelated operation failed",
                    detail: None,
                },
            );
            device_browser::show_catalog_error(ui, &intl, error.as_deref());
        },
        Some("The synthetic device is busy".to_owned()),
    );
    view.run();
    let _ = view.get_by_label("Could not browse device files");
    let _ = view.get_by_label("The synthetic device is busy");
    *view.state_mut() = None;
    view.run();
    assert!(
        view.query_by_label("Could not browse device files")
            .is_none()
    );
    assert!(
        view.query_by_label("The synthetic device is busy")
            .is_none()
    );
    let _ = view.get_by_label("An unrelated operation failed");
}
