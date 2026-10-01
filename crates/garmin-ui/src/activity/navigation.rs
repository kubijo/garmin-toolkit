//! Calendar navigation and an on-demand archive, shared by every activity host.

use egui::{RichText, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_model::activity::ActivitySport;

use super::{Action, Presentation, WorkspaceProps, calendar};
use crate::{Size, button, icons, input, select};

#[derive(Default)]
pub(super) struct Navigation {
    calendar: calendar::Calendar,
    archive_open: bool,
    query: String,
    sport: Option<ActivitySport>,
}

impl Navigation {
    pub(super) fn open_archive(&mut self) {
        self.archive_open = true;
    }

    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        props: &WorkspaceProps<'_>,
        actions: &mut impl FnMut(&mut Ui),
    ) -> Option<Action> {
        let mut action =
            self.calendar
                .show(ui, intl, props.presentations, props.selected, props.today);
        ui.add_space(8.0);
        action = self.details(ui, intl, props).or(action);
        actions(ui);
        action
    }

    fn details(&mut self, ui: &mut Ui, intl: &Intl, props: &WorkspaceProps<'_>) -> Option<Action> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if let Some(presentation) = props.selected.and_then(|i| props.presentations.get(i)) {
                let heading = ui.label(RichText::new(&presentation.title).size(18.0).strong());
                crate::semantics::target(ui, &heading, "activity.selection");
                crate::semantics::value(&heading, props.selected.unwrap_or_default().to_string());
            }
            action = self.controls(ui, intl, props);
        });
        let Some(presentation) = props
            .selected
            .and_then(|index| props.presentations.get(index))
        else {
            return action;
        };
        ui.label(RichText::new(&presentation.subtitle).small());
        ui.add_space(4.0);
        metrics(ui, presentation);
        let day = calendar::on_day(props.presentations, presentation.local_start.date());
        if day.len() > 1 {
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("activity-day-list")
                .max_height(80.0)
                .show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 24.0;
                    ui.spacing_mut().button_padding.y = 2.0;
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for index in day {
                        let item = &props.presentations[index];
                        let label = format!(
                            "{} · {} · {}",
                            calendar::time_label(intl, item.local_start),
                            item.title,
                            item.distance.as_deref().unwrap_or(&item.duration)
                        );
                        let response = ui.add(
                            egui::Button::selectable(props.selected == Some(index), ())
                                .left_text(label)
                                .gap(0.0)
                                .min_size(egui::vec2(ui.available_width(), 24.0)),
                        );
                        if props.selected == Some(index) {
                            let background = ui.visuals().selection.bg_fill;
                            let surface = garmin_color::Color::from_u32(u32::from_be_bytes(
                                background.to_srgba_unmultiplied(),
                            ));
                            let accent = crate::theme::color32(crate::theme::selection_accent_on(
                                ui, surface,
                            ));
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    response.rect.min,
                                    egui::vec2(2.0, response.rect.height()),
                                ),
                                egui::CornerRadius::ZERO,
                                accent,
                            );
                        }
                        crate::semantics::target(ui, &response, format!("activity.day.{index}"));
                        if response.clicked() {
                            action = Some(Action::Select(index));
                        }
                    }
                });
        }
        action
    }

    fn controls(&mut self, ui: &mut Ui, intl: &Intl, props: &WorkspaceProps<'_>) -> Option<Action> {
        let order = calendar::chronological(props.presentations);
        let position = props
            .selected
            .and_then(|selected| order.iter().position(|&index| index == selected));
        let previous = position
            .and_then(|p| p.checked_sub(1))
            .and_then(|p| order.get(p))
            .copied();
        let next = position.and_then(|p| order.get(p + 1)).copied();
        let mut action = None;
        for (label, target, index, icon) in [
            (
                format_message!(intl, default_message: "Previous activity"),
                "activity.previous",
                previous,
                icons::CARET_LEFT,
            ),
            (
                format_message!(intl, default_message: "Next activity"),
                "activity.next",
                next,
                icons::CARET_RIGHT,
            ),
        ] {
            let response = button::IconProps {
                icon,
                label: &label,
                kind: button::Kind::Ghost,
                size: Size::Small,
                enabled: index.is_some(),
            }
            .show(ui);
            crate::semantics::target(ui, &response, target);
            if response.clicked() {
                action = index.map(Action::Select);
            }
        }
        let response = button::Props {
            label: &format_message!(intl, default_message: "All activities"),
            icon: None,
            kind: button::Kind::Ghost,
            size: Size::Small,
            width: button::Width::Fill,
            enabled: true,
        }
        .show(ui);
        if response.clicked() {
            self.archive_open = true;
        }
        crate::semantics::target(ui, &response, "activity.list.toggle");
        crate::semantics::value(&response, if self.archive_open { "open" } else { "closed" });
        action
    }

    pub(super) fn archive(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        props: &WorkspaceProps<'_>,
    ) -> Option<Action> {
        if !self.archive_open {
            return None;
        }
        let mut action = None;
        let mut close = false;
        let width = (ui.ctx().content_rect().width() - 48.0).clamp(1.0, 560.0);
        let style = ui.style().clone();
        let accent = crate::theme::profile_accent(ui);
        let response = egui::Modal::new(ui.id().with("activity-archive"))
            .frame(crate::modal::surface_frame(ui).inner_margin(16))
            .backdrop_color(crate::theme::color32(crate::theme::palette(ui).overlay()))
            .show(ui.ctx(), |ui| {
                crate::theme::with_profile_accent(ui, accent, |ui| {
                    ui.set_style(style);
                    ui.set_width(width);
                    ui.heading(format_message!(intl, default_message: "All activities"));
                    self.archive_filters(ui, intl);
                    let query = self.query.to_lowercase();
                    egui::ScrollArea::vertical()
                        .max_height((ui.ctx().content_rect().height() - 180.0).max(80.0))
                        .show(ui, |ui| {
                            let mut any = false;
                            for index in calendar::chronological(props.presentations)
                                .into_iter()
                                .rev()
                            {
                                let item = &props.presentations[index];
                                if !matches_query(item, self.sport, &query) {
                                    continue;
                                }
                                any = true;
                                if archive_row(ui, item, index, props.selected == Some(index)) {
                                    action = Some(Action::Select(index));
                                }
                            }
                            if !any {
                                ui.label(
                                format_message!(intl, default_message: "No matching activities"),
                            );
                            }
                        });
                    let response = ui.button(format_message!(intl, default_message: "Close"));
                    crate::semantics::target(ui, &response, "activity.list.close");
                    close = response.clicked();
                });
            });
        if response.should_close() || close || action.is_some() {
            self.archive_open = false;
        }
        action
    }

    fn archive_filters(&mut self, ui: &mut Ui, intl: &Intl) {
        let sports = [
            None,
            Some(ActivitySport::Cycling),
            Some(ActivitySport::Running),
            Some(ActivitySport::Swimming),
        ];
        let labels = sports.map(|sport| {
            sport.map_or_else(
                || format_message!(intl, default_message: "All sports"),
                |sport| super::sport_title(sport, intl),
            )
        });
        let choices = labels.each_ref().map(|label| select::Choice::new(label));
        let mut selected = sports
            .iter()
            .position(|sport| *sport == self.sport)
            .unwrap_or(0);
        ui.horizontal_top(|ui| {
            let filter_width = 144.0;
            let search_width =
                (ui.available_width() - filter_width - ui.spacing().item_spacing.x).max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(search_width, 32.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let search = input::show(
                        ui,
                        &mut self.query,
                        input::Props::new("")
                            .placeholder(
                                &format_message!(intl, default_message: "Search activities"),
                            )
                            .size(Size::Small),
                    );
                    crate::semantics::target(ui, &search, "activity.archive.search");
                },
            );
            ui.vertical(|ui| {
                let response = select::show(
                    ui,
                    ui.id().with("activity-sport"),
                    &mut selected,
                    &choices,
                    select::Props::new("").size(Size::Small).width(filter_width),
                );
                crate::semantics::target(ui, &response, "activity.archive.sport");
            });
        });
        self.sport = sports[selected];
    }
}

fn matches_query(item: &Presentation, sport: Option<ActivitySport>, query: &str) -> bool {
    sport.is_none_or(|sport| item.sport == sport)
        && (item.title.to_lowercase().contains(query)
            || item.subtitle.to_lowercase().contains(query)
            || item.local_start.date().to_string().contains(query))
}

fn archive_row(ui: &mut Ui, item: &Presentation, index: usize, selected: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), super::ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let response = super::row_response(ui, rect, index, &item.title);
    crate::semantics::value(&response, if selected { "selected" } else { "unselected" });
    super::paint_row(ui, rect, &item.item_props(), selected, &response);
    response.clicked()
}

fn metrics(ui: &mut Ui, presentation: &Presentation) {
    if presentation.metrics.is_empty() {
        return;
    }
    ui.columns(presentation.metrics.len(), |columns| {
        for (ui, metric) in columns.iter_mut().zip(&presentation.metrics) {
            ui.label(RichText::new(&metric.label).small());
            ui.label(RichText::new(&metric.value).size(16.0));
        }
    });
}
