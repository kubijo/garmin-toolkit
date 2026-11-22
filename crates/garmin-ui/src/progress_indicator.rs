//! Ordered steps in a workflow. The caller owns the current step and navigation.

use egui::emath::Numeric as _;
use egui::{Color32, Rect, Sense, Ui, pos2, vec2};
use garmin_i18n::{Intl, format_message};

use crate::{icons, skeleton::Skeleton, theme, typography};

const SEGMENT_GAP: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Complete,
    Current,
    Upcoming,
    Invalid,
    Disabled,
}

impl Status {
    const fn color(self, palette: &garmin_color::theme::Theme) -> garmin_color::Color {
        match self {
            Self::Complete | Self::Current => palette.support().information(),
            Self::Invalid => palette.support().error(),
            Self::Upcoming => palette.content().text_helper(),
            Self::Disabled => palette.content().text_disabled(),
        }
    }

    fn name(self, intl: &Intl) -> String {
        match self {
            Self::Complete => format_message!(intl, default_message: "Complete"),
            Self::Current => format_message!(intl, default_message: "Current step"),
            Self::Upcoming => format_message!(intl, default_message: "Upcoming"),
            Self::Invalid => format_message!(intl, default_message: "Needs attention"),
            Self::Disabled => format_message!(intl, default_message: "Unavailable"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Step<'a> {
    pub label: &'a str,
    pub description: Option<&'a str>,
    pub status: Status,
}

pub struct Props<'a> {
    pub steps: &'a [Step<'a>],
    /// Enable navigation to available steps. A click returns the requested index.
    pub interactive: bool,
}

/// Places steps in columns when they fit and stacks them on narrow surfaces.
#[must_use]
pub fn show(ui: &mut Ui, intl: &Intl, props: &Props<'_>) -> Option<usize> {
    let mut selected = None;
    if !props.steps.is_empty() {
        let vertical = stack_steps(ui, props.steps.len());
        let mut draw = |ui: &mut Ui, index: usize, step: &Step<'_>| {
            if draw_step(ui, intl, step, props.interactive) {
                selected = Some(index);
            }
        };
        if vertical {
            for (index, step) in props.steps.iter().enumerate() {
                draw(ui, index, step);
            }
        } else {
            let palette = theme::palette(ui);
            rail(
                ui,
                props
                    .steps
                    .iter()
                    .map(|step| theme::color32(step.status.color(palette))),
            );
            step_columns(ui, props.steps.len(), |columns| {
                for (index, (column, step)) in columns.iter_mut().zip(props.steps).enumerate() {
                    draw(column, index, step);
                }
            });
        }
    }
    selected
}

fn stack_steps(ui: &Ui, count: usize) -> bool {
    f64::from(ui.available_width() + SEGMENT_GAP) < count.to_f64() * f64::from(96.0 + SEGMENT_GAP)
}

fn step_columns(ui: &mut Ui, count: usize, draw: impl FnOnce(&mut [Ui])) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = SEGMENT_GAP;
        ui.columns(count, draw);
    });
}

// Snap each shared boundary once so neighboring segments always leave the full gap.
fn rail(ui: &mut Ui, colors: impl ExactSizeIterator<Item = Color32>) {
    let count = colors.len().to_f64();
    let (track, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.0), Sense::hover());
    let scale = ui.ctx().pixels_per_point();
    let boundary = |i: usize| {
        let fraction = f32::from_f64(i.to_f64() / count);
        ((track.left() + (track.width() + SEGMENT_GAP) * fraction) * scale).round() / scale
    };
    for (i, color) in colors.enumerate() {
        let segment = Rect::from_min_max(
            pos2(boundary(i), track.top()),
            pos2(boundary(i + 1) - SEGMENT_GAP, track.bottom()),
        );
        ui.painter().rect_filled(segment, 0, color);
    }
}

fn draw_step(ui: &mut Ui, intl: &Intl, step: &Step<'_>, interactive: bool) -> bool {
    let mark_color = step.status.color(theme::palette(ui));
    let color = theme::color32(mark_color);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let height = if interactive { 24.0 } else { 20.0 };
        let (mark, _) = ui.allocate_exact_size(vec2(16.0, height), Sense::hover());
        let center = mark.center();
        ui.painter()
            .circle_stroke(center, 6.0, egui::Stroke::new(1.0, color));
        match step.status {
            Status::Current => {
                ui.painter().circle_filled(center, 3.0, color);
            }
            Status::Complete | Status::Invalid => {
                let icon = if step.status == Status::Complete {
                    icons::CHECK
                } else {
                    icons::X
                };
                icons::Props {
                    icon,
                    size: 12.0,
                    color: mark_color,
                }
                .paint_at(ui, center);
            }
            Status::Disabled => {
                let diagonal = vec2(4.0, -4.0);
                ui.painter().line_segment(
                    [center - diagonal, center + diagonal],
                    egui::Stroke::new(1.0, color),
                );
            }
            Status::Upcoming => {}
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let text = match step.status {
                Status::Current => typography::semibold(step.label),
                Status::Disabled => egui::RichText::new(step.label).weak(),
                _ => egui::RichText::new(step.label),
            };
            let response = if interactive {
                ui.spacing_mut().interact_size.y = height;
                ui.add_enabled(
                    step.status != Status::Disabled,
                    egui::Button::new(text).frame(false).wrap(),
                )
            } else {
                ui.add(egui::Label::new(text).wrap().selectable(false))
            };
            let mut description = step.status.name(intl);
            if let Some(detail) = step.description {
                typography::helper(ui, detail);
                description.push_str(". ");
                description.push_str(detail);
            }
            response
                .ctx
                .accesskit_node_builder(response.id, |node| node.set_description(description));
            response.clicked() && interactive
        })
        .inner
    })
    .inner
}

/// Placeholder for an indicator whose steps are loading.
pub fn skeleton(ui: &mut Ui, count: usize) {
    if count == 0 {
        return;
    }
    let vertical = stack_steps(ui, count);
    let palette = theme::palette(ui);
    let placeholder = |ui: &mut Ui| {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
        icons::Props {
            icon: icons::CIRCLE_DASHED,
            size: 16.0,
            color: palette.content().text_helper(),
        }
        .paint_at(ui, rect.left_center() + vec2(8.0, 0.0));
        Skeleton::new(vec2((width - 24.0).clamp(0.0, 36.0), 12.0))
            .paint_at(ui, rect.left_center() + vec2(24.0, -6.0));
    };
    if vertical {
        for _ in 0..count {
            placeholder(ui);
        }
    } else {
        let color = theme::color32(palette.borders().subtle());
        rail(ui, std::iter::repeat_n(color, count));
        step_columns(ui, count, |columns| {
            columns.iter_mut().for_each(placeholder);
        });
    }
}
