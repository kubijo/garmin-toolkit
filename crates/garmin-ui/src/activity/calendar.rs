//! Local-date navigation; activity indices always refer to the host's original list.

use egui::{RichText, Ui};
use garmin_i18n::{CalendarDay, Intl, MonthGrid, format_message};
use jiff::civil::{Date, DateTime};

use super::{Action, Presentation};
use crate::{Size, button, icons};

const CELL: f32 = 24.0;

#[derive(Default)]
pub(super) struct Calendar {
    month: Option<Date>,
    selection: Option<(usize, jiff::Timestamp)>,
    grid: Option<CachedMonth>,
}

struct CachedMonth {
    anchor: Date,
    locale: String,
    model: MonthGrid,
}

impl CachedMonth {
    fn new(anchor: Date, intl: &Intl) -> Self {
        Self {
            anchor,
            locale: intl.dates().locale().to_owned(),
            model: intl.dates().month(anchor),
        }
    }
}

impl Calendar {
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        activities: &[Presentation],
        selected: Option<usize>,
        today: Date,
    ) -> Option<Action> {
        self.sync(activities, selected);
        let anchor = self.month.unwrap_or(today);
        let locale = intl.dates().locale();
        let grid = self
            .grid
            .get_or_insert_with(|| CachedMonth::new(anchor, intl));
        if grid.anchor != anchor || grid.locale != locale {
            *grid = CachedMonth::new(anchor, intl);
        }
        if let Some(month) = month_header(ui, intl, &grid.model) {
            self.month = Some(month);
            *grid = CachedMonth::new(month, intl);
        }
        let selected_day = selected
            .and_then(|index| activities.get(index))
            .map(|p| p.local_start.date());
        let mut action = None;
        let cell_width = ((ui.available_width() - 12.0) / 7.0).max(CELL);
        egui::Grid::new("activity-calendar")
            .spacing(egui::Vec2::splat(2.0))
            .min_col_width(cell_width)
            .min_row_height(0.0)
            .show(ui, |ui| {
                ui.spacing_mut().interact_size = egui::Vec2::splat(CELL);
                ui.spacing_mut().button_padding = egui::Vec2::splat(2.0);
                for weekday in &grid.model.weekdays {
                    ui.add_sized(
                        [cell_width, 18.0],
                        egui::Label::new(RichText::new(weekday).small()),
                    );
                }
                ui.end_row();
                for row in grid.model.days.chunks(7) {
                    for day in row {
                        if let Some(day) = day {
                            if let Some(index) =
                                day_button(ui, day, selected_day, activities, selected, cell_width)
                            {
                                action = Some(Action::Select(index));
                            }
                        } else {
                            ui.allocate_space(egui::vec2(cell_width, CELL));
                        }
                    }
                    ui.end_row();
                }
            });
        action
    }

    fn sync(&mut self, activities: &[Presentation], selected: Option<usize>) {
        let selection =
            selected.and_then(|index| activities.get(index).map(|p| (index, p.started_at)));
        if self.selection != selection {
            self.selection = selection;
            if let Some((index, _)) = selection {
                self.month = Some(activities[index].local_start.date());
            }
        }
    }
}

fn month_header(ui: &mut Ui, intl: &Intl, month: &MonthGrid) -> Option<Date> {
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
        crate::semantics::target(ui, &response, "activity.calendar.previous-month");
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
        crate::semantics::target(ui, &response, "activity.calendar.next-month");
        if response.clicked() {
            selected = next;
        }
    });
    selected
}

fn day_button(
    ui: &mut Ui,
    cell: &CalendarDay,
    selected_day: Option<Date>,
    activities: &[Presentation],
    selected: Option<usize>,
    width: f32,
) -> Option<usize> {
    let day = cell.date;
    let index = select_day(activities, selected, day);
    let is_selected = selected_day == Some(day);
    let tile = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .push_id(day, |ui| {
            ui.add_enabled(
                index.is_some(),
                egui::Button::new(&cell.label)
                    .selected(is_selected)
                    .frame(false)
                    .min_size(egui::vec2(width, CELL)),
            )
        })
        .inner
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if !response.enabled() && response.contains_pointer() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
    }
    let palette = crate::theme::palette(ui);
    let contrast = if is_selected { 0.16 } else { 0.08 };
    let emphasis = if response.is_pointer_button_down_on() {
        0.12
    } else if response.hovered() {
        0.08
    } else {
        0.0
    };
    let fill = crate::theme::color32(palette.surfaces().background()).lerp_to_gamma(
        crate::theme::color32(palette.content().text_primary()),
        contrast + emphasis,
    );
    ui.painter().set(
        tile,
        egui::Shape::rect_filled(response.rect, egui::CornerRadius::ZERO, fill),
    );
    if is_selected {
        let [r, g, b, _] = fill.to_srgba_unmultiplied();
        ui.painter().line_segment(
            [
                response.rect.left_bottom() - egui::vec2(0.0, 1.0),
                response.rect.right_bottom() - egui::vec2(0.0, 1.0),
            ],
            egui::Stroke::new(
                2.0,
                crate::theme::color32(crate::theme::selection_accent_on(
                    ui,
                    garmin_color::Color::from_rgb(r, g, b),
                )),
            ),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(2.0, crate::theme::color32(palette.interaction().focus())),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            index.is_some(),
            selected_day == Some(day),
            &cell.description,
        )
    });
    crate::semantics::target(ui, &response, format!("activity.calendar.{day}"));
    crate::semantics::value(
        &response,
        if selected_day == Some(day) {
            "selected"
        } else {
            "unselected"
        },
    );
    if index.is_some() {
        ui.painter().circle_filled(
            response.rect.center_bottom() - egui::vec2(0.0, 4.0),
            1.5,
            ui.visuals().selection.stroke.color,
        );
    }
    response.clicked().then_some(index).flatten()
}

pub(super) fn chronological(activities: &[Presentation]) -> Vec<usize> {
    let mut indices = (0..activities.len()).collect::<Vec<_>>();
    indices.sort_by_key(|&index| (activities[index].started_at, index));
    indices
}

pub(super) fn on_day(activities: &[Presentation], day: Date) -> Vec<usize> {
    chronological(activities)
        .into_iter()
        .filter(|&index| activities[index].local_start.date() == day)
        .collect()
}

fn select_day(activities: &[Presentation], selected: Option<usize>, day: Date) -> Option<usize> {
    selected
        .filter(|&index| {
            activities
                .get(index)
                .is_some_and(|p| p.local_start.date() == day)
        })
        .or_else(|| {
            activities
                .iter()
                .enumerate()
                .filter(|(_, p)| p.local_start.date() == day)
                .min_by_key(|(index, p)| (p.started_at, *index))
                .map(|(index, _)| index)
        })
}

pub(super) fn date_time_label(intl: &Intl, local: DateTime) -> String {
    intl.dates().datetime(local)
}

pub(super) fn time_label(intl: &Intl, local: DateTime) -> String {
    intl.dates().time(local.time())
}

#[cfg(test)]
mod tests;
