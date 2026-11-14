//! Shared locale-aware calendar navigation.

use crate::{Size, button, icons};
use egui::{RichText, Ui};
use garmin_i18n::{Intl, MonthGrid, format_message};
use jiff::civil::Date;

pub(crate) fn month_header(
    ui: &mut Ui,
    intl: &Intl,
    month: &MonthGrid,
    target: &str,
) -> Option<Date> {
    let mut selected = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let previous = month.previous;
        let next = month.next;
        let response = button::IconProps {
            icon: icons::CARET_LEFT,
            label: &format_message!(intl, default_message: "Previous month"),
            kind: button::Kind::Ghost,
            size: Size::Small,
            enabled: previous.is_some(),
        }
        .show(ui);
        crate::semantics::target(ui, &response, format!("{target}.previous-month"));
        if response.clicked() {
            selected = previous;
        }
        ui.add_sized(
            [(ui.available_width() - 36.0).max(1.0), 32.0],
            egui::Label::new(RichText::new(&month.title).strong()),
        );
        let response = button::IconProps {
            icon: icons::CARET_RIGHT,
            label: &format_message!(intl, default_message: "Next month"),
            kind: button::Kind::Ghost,
            size: Size::Small,
            enabled: next.is_some(),
        }
        .show(ui);
        crate::semantics::target(ui, &response, format!("{target}.next-month"));
        if response.clicked() {
            selected = next;
        }
    });
    selected
}
