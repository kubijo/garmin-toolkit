//! Related fact tables retain their alignment when labels or available width change.

use egui_kittest::{Harness, kittest::Queryable as _};
use garmin_ui::facts;

#[test]
fn wrapped_rows_start_at_the_table_top_and_keep_the_next_row_clear() {
    let mut installed = false;
    let mut view = Harness::builder().with_size([280.0, 300.0]).build_ui_state(
        move |ui, top: &mut f32| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            ui.set_width(220.0);
            *top = ui.next_widget_position().y;
            facts::show(
                ui,
                "wrapped",
                &[
                    ("ID\u{a0}toolkitu", "9e132b21-4910-47bd-93ac-1ea7a9afb2da"),
                    ("State", "Ready"),
                ],
            );
        },
        0.0,
    );
    view.run();
    let label = view.get_by_label("ID\u{a0}toolkitu").rect();
    let value = view
        .get_by_label("9e132b21-4910-47bd-93ac-1ea7a9afb2da")
        .rect();
    assert!((label.top() - view.state()).abs() < 1.0);
    assert!((value.top() - label.top()).abs() < 1.0);
    assert!(value.height() > label.height());
    assert!(view.get_by_label("Ready").rect().top() >= value.bottom() + 7.0);
}

#[test]
fn sections_keep_separate_aligned_columns_after_locale_and_width_changes() {
    let mut installed = false;
    let mut view = Harness::builder().with_size([400.0, 400.0]).build_ui_state(
        move |ui, state: &mut (bool, f32)| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            ui.set_width(state.1);
            let labels = if state.0 {
                ["ID zařízení", "Tréninky"]
            } else {
                ["Device ID", "Workouts"]
            };
            let table = facts::Table::new(ui, labels);
            table.show(ui, "summary", &[(labels[0], "42530200")]);
            ui.add_space(16.0);
            table.show(
                ui,
                "transfers",
                &[(labels[1].to_owned(), "Read and write".to_owned())],
            );
        },
        (false, 360.0),
    );
    for (czech, width) in [(false, 360.0), (true, 288.0), (true, 160.0), (false, 360.0)] {
        *view.state_mut() = (czech, width);
        view.run();
        let identifier = view
            .get_by_label(if czech { "ID zařízení" } else { "Device ID" })
            .rect();
        let transfer = view
            .get_by_label(if czech { "Tréninky" } else { "Workouts" })
            .rect();
        let id_value = view.get_by_label("42530200").rect();
        let transfer_value = view.get_by_label("Read and write").rect();
        assert!((identifier.right() - transfer.right()).abs() < 1.0);
        assert!((id_value.left() - transfer_value.left()).abs() < 1.0);
        assert!(id_value.left() - identifier.right() >= 7.0);
        assert!(transfer_value.left() - transfer.right() >= 7.0);
    }
}
