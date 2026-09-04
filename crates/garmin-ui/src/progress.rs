//! Operation progress.

use cint::ColorInterop;
use egui::{RichText, Sense, Ui, emath::Numeric};

use crate::theme as widget_theme;

/// Progress value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value {
    Indeterminate,
    Determinate {
        /// Completed units.
        completed: usize,
        /// Total units.
        total: usize,
    },
}

/// Progress-bar inputs.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub label: &'a str,
    pub detail: Option<&'a str>,
    pub value: Value,
}

/// Renders a thin operation progress bar.
pub fn show(ui: &mut Ui, props: &Props<'_>) {
    let palette = crate::theme::palette(ui);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.add(
            egui::Label::new(
                RichText::new(props.label).color(palette.content().text_primary().into_cint()),
            )
            .selectable(false),
        );
        match props.value {
            Value::Indeterminate => indeterminate(ui, palette),
            Value::Determinate { completed, total } => {
                let fraction = if total == 0 {
                    1.0
                } else {
                    f32::from_f64(completed.to_f64() / total.to_f64())
                };
                ui.add(
                    widget_theme::progress_bar(fraction, palette.interaction().interactive())
                        .desired_height(4.0)
                        .corner_radius(egui::CornerRadius::ZERO),
                );
            }
        }
        if let Some(detail) = props.detail {
            ui.add(
                egui::Label::new(
                    RichText::new(detail)
                        .size(12.0)
                        .color(palette.content().text_helper().into_cint()),
                )
                .selectable(false),
            );
        }
    });
}

fn indeterminate(ui: &mut Ui, palette: &garmin_color::theme::Theme) {
    let width = ui.available_width().max(0.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 4.0), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator));
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

    let segment_width = rect.width() * 0.32;
    let time = ui.input(|input| input.time);
    let phase = f32::from_f64((time / 1.2).rem_euclid(1.0));
    let left = egui::lerp((rect.left() - segment_width)..=rect.right(), phase);
    let segment = egui::Rect::from_min_size(
        egui::pos2(left, rect.top()),
        egui::vec2(segment_width, rect.height()),
    );
    ui.painter().with_clip_rect(rect).rect_filled(
        segment,
        0.0,
        widget_theme::color32(palette.interaction().interactive()),
    );
    ui.request_repaint();
}
