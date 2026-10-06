use egui::{Response, Ui};
use garmin_color::theme::Level;

use super::State;
use crate::{Size, button, icons, semantics, theme, typography};

pub(super) struct Row<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub icon: icons::Icon,
    pub selected: bool,
    pub enabled: bool,
    pub target: &'a str,
}

impl Row<'_> {
    pub fn show(&self, ui: &mut Ui) -> Response {
        ui.push_id(self.target, |ui| {
            ui.add_enabled_ui(self.enabled, |ui| self.content(ui)).inner
        })
        .inner
    }

    fn content(&self, ui: &mut Ui) -> Response {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 72.0), egui::Sense::click());
        let response = button::interaction_cursor(response).on_hover_text(self.title);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("{} · {}", self.title, self.subtitle),
            )
        });
        semantics::target(ui, &response, self.target);
        let palette = theme::palette(ui);
        let fill = if response.hovered() || self.selected {
            palette.surfaces().layer_hover(Level::One)
        } else {
            palette.surfaces().layer(Level::One)
        };
        ui.painter().rect_filled(rect, 0, theme::color32(fill));
        if self.selected {
            let marker = egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height()));
            ui.painter()
                .rect_filled(marker, 0, theme::color32(theme::selection_accent(ui)));
        }
        let primary = if ui.is_enabled() {
            palette.content().text_primary()
        } else {
            palette.content().text_disabled()
        };
        let secondary = if ui.is_enabled() {
            palette.content().text_secondary()
        } else {
            palette.content().text_disabled()
        };
        icons::Props {
            icon: self.icon,
            size: 24.0,
            color: secondary,
        }
        .paint_at(ui, egui::pos2(rect.left() + 28.0, rect.center().y));
        icons::Props {
            icon: if self.selected {
                icons::CHECK
            } else {
                icons::CARET_RIGHT
            },
            size: 16.0,
            color: secondary,
        }
        .paint_at(ui, egui::pos2(rect.right() - 24.0, rect.center().y));
        let text_width = (rect.width() - 104.0).max(0.0);
        for (label, weight, color, y) in [
            (self.title, typography::Weight::SemiBold, primary, 16.0),
            (self.subtitle, typography::Weight::Regular, secondary, 40.0),
        ] {
            let mut job = egui::text::LayoutJob::simple(
                label.to_owned(),
                typography::font(14.0, weight),
                theme::color32(color),
                text_width,
            );
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            ui.painter().galley(
                rect.min + egui::vec2(56.0, y),
                galley,
                theme::color32(color),
            );
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(2.0),
                0,
                egui::Stroke::new(2.0, theme::color32(palette.interaction().focus())),
                egui::StrokeKind::Inside,
            );
        }
        response
    }
}

pub(super) fn button(label: &str, kind: button::Kind, enabled: bool) -> button::Props<'_> {
    button::Props {
        label,
        icon: None,
        kind,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
}

pub(super) fn action_button(
    ui: &mut Ui,
    id: &str,
    label: &str,
    kind: button::Kind,
    enabled: bool,
) -> bool {
    let response = button(label, kind, enabled).show(ui);
    semantics::target(ui, &response, id);
    response.clicked()
}

pub(super) fn icon_button(
    ui: &mut Ui,
    id: &str,
    label: &str,
    icon: icons::Icon,
    enabled: bool,
) -> bool {
    let response = button::IconProps {
        label,
        icon,
        kind: button::Kind::Tertiary,
        size: Size::Medium,
        enabled,
    }
    .show(ui);
    semantics::target(ui, &response, id);
    response.clicked()
}

pub(super) fn action_row<A, const N: usize>(
    ui: &mut Ui,
    buttons: [(&str, A, button::Props<'_>); N],
) -> Option<A> {
    let width = buttons
        .iter()
        .map(|(_, _, button)| button.natural_width(ui))
        .sum::<f32>()
        + 8.0 * f32::from(u16::try_from(N.saturating_sub(1)).unwrap_or(u16::MAX));
    let stacked = width > ui.available_width();
    let mut action = None;
    let show = |ui: &mut Ui| {
        for (id, requested, mut button) in buttons {
            if stacked {
                button.width = button::Width::Fill;
            }
            let response = button.show(ui);
            semantics::target(ui, &response, id);
            if response.clicked() {
                action = Some(requested);
            }
        }
    };
    if stacked {
        ui.vertical(show);
    } else {
        ui.horizontal(show);
    }
    action
}

pub(super) fn busy(ui: &mut Ui, state: &State) {
    ui.allocate_ui(egui::vec2(24.0, 24.0), |ui| {
        if state.busy || state.pending.is_some() {
            ui.spinner();
        }
    });
}
