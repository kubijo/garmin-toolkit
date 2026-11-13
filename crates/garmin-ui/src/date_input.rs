//! Editable ISO date field with a locale-aware calendar popup.

use egui::{Id, Popup, Response, RichText, Ui};
use garmin_i18n::{Intl, format_message};
use jiff::civil::Date;

use crate::{Size, button, icons, input};

/// Renders a date input. Empty values remain empty until a day is chosen or typed.
pub fn show(
    ui: &mut Ui,
    id: Id,
    value: &mut String,
    intl: &Intl,
    label: &str,
    enabled: bool,
    today: Date,
) -> Response {
    let choose =
        format_message!(intl, default_message: "Choose date for {field}", values: { field: label });
    let mut response = input::show(
        ui,
        value,
        input::Props::new(label)
            .placeholder("YYYY-MM-DD")
            .disabled(!enabled)
            .action(button::IconProps {
                icon: icons::CALENDAR,
                label: &choose,
                kind: button::Kind::Ghost,
                size: Size::Medium,
                enabled,
            }),
    );
    let popup_id = id.with("calendar-popup");
    let month_id = id.with("calendar-month");
    let selected = value.trim().parse::<Date>().ok();
    if response.clicked() && !Popup::is_id_open(ui.ctx(), popup_id) {
        ui.data_mut(|data| data.insert_temp(month_id, selected.unwrap_or(today)));
    }
    let mut changed = false;
    let _ = Popup::menu(&response)
        .id(popup_id)
        .width(280.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let mut month = ui
                .data(|data| data.get_temp::<Date>(month_id))
                .unwrap_or(today);
            if let Some(date) = calendar(ui, intl, &mut month, selected) {
                *value = date.to_string();
                changed = true;
                ui.close();
            }
            ui.data_mut(|data| data.insert_temp(month_id, month));
        });
    if changed {
        response.mark_changed();
    }
    response
}

fn calendar(ui: &mut Ui, intl: &Intl, month: &mut Date, selected: Option<Date>) -> Option<Date> {
    let mut grid = intl.dates().month(*month);
    if let Some(next) = crate::calendar::month_header(ui, intl, &grid, "date-input.calendar") {
        *month = next;
        grid = intl.dates().month(next);
    }
    let mut picked = None;
    egui::Grid::new("date-picker-days")
        .spacing(egui::Vec2::splat(4.0))
        .min_col_width(40.0)
        .min_row_height(0.0)
        .show(ui, |ui| {
            for weekday in &grid.weekdays {
                ui.add_sized(
                    [40.0, 24.0],
                    egui::Label::new(RichText::new(weekday).size(12.0)),
                );
            }
            ui.end_row();
            for week in grid
                .days
                .chunks(7)
                .filter(|week| week.iter().any(Option::is_some))
            {
                for day in week {
                    if let Some(day) = day {
                        let response = ui
                            .allocate_ui(egui::vec2(40.0, 32.0), |ui| {
                                button::Props {
                                    label: &day.label,
                                    icon: None,
                                    kind: if selected == Some(day.date) {
                                        button::Kind::Primary
                                    } else {
                                        button::Kind::Ghost
                                    },
                                    size: Size::Small,
                                    width: button::Width::Fill,
                                    enabled: true,
                                }
                                .show(ui)
                            })
                            .inner;
                        if selected != Some(day.date) {
                            ui.painter().rect_stroke(
                                response.rect,
                                egui::CornerRadius::ZERO,
                                egui::Stroke::new(
                                    1.0,
                                    crate::theme::color32(
                                        crate::theme::palette(ui).borders().subtle(),
                                    ),
                                ),
                                egui::StrokeKind::Inside,
                            );
                        }
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::Button,
                                true,
                                selected == Some(day.date),
                                &day.description,
                            )
                        });
                        crate::semantics::target(
                            ui,
                            &response,
                            format!("date-input.day.{}", day.date),
                        );
                        if response.clicked() {
                            picked = Some(day.date);
                        }
                    } else {
                        ui.allocate_space(egui::vec2(40.0, 32.0));
                    }
                }
                ui.end_row();
            }
        });
    picked
}
