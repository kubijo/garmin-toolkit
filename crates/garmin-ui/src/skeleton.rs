//! Animated placeholders for content whose dimensions are known while loading.

use std::time::Duration;

use egui::emath::Numeric as _;
use egui::{CornerRadius, Pos2, Rect, Ui, Vec2};

use crate::theme;

const CYCLE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
#[must_use = "call show or paint_at to draw the skeleton"]
pub struct Skeleton {
    size: Vec2,
    circular: bool,
}

impl Skeleton {
    pub const fn new(size: Vec2) -> Self {
        Self {
            size,
            circular: false,
        }
    }

    pub fn circle(diameter: f32) -> Self {
        Self {
            size: Vec2::splat(diameter),
            circular: true,
        }
    }

    /// Reserve the placeholder's dimensions in the current layout.
    pub fn show(self, ui: &mut Ui) {
        let (_, rect) = ui.allocate_space(self.size);
        self.paint_at(ui, rect.min);
    }

    /// Paint within an existing layout without allocating space or handling input.
    pub fn paint_at(self, ui: &Ui, origin: Pos2) {
        let rect = Rect::from_min_size(origin, self.size);
        if !rect.is_positive() || !ui.is_rect_visible(rect) {
            return;
        }
        let ink = theme::palette(ui).content().text_primary();
        let base = theme::color32(ink.with_alpha(0x12));
        let element = theme::color32(ink.with_alpha(0x0f));
        let paint = |painter: &egui::Painter, color| {
            if self.circular {
                painter.circle_filled(rect.center(), rect.width() / 2.0, color);
            } else {
                painter.rect_filled(rect, CornerRadius::ZERO, color);
            }
        };
        paint(ui.painter(), base);
        if ui.style().animation_time <= 0.0 {
            return;
        }
        let phase = f32::from_f64((ui.input(|i| i.time) / CYCLE.as_secs_f64()).rem_euclid(1.0));
        let (start, end, opacity) = sweep(phase);
        let width = if self.circular { 2.0 } else { 1.0 } * rect.width();
        let mut band = rect;
        band.min.x += start * width;
        band.max.x = rect.left() + end * width;
        if band.is_positive() {
            paint(
                &ui.painter().with_clip_rect(band),
                element.gamma_multiply(opacity),
            );
        }
        ui.ctx().request_repaint();
    }
}

// Carbon's skeleton expands from the left, collapses to the right, then reverses.
// Opacity has its own keyframes at 0%, 20%, and 100%.
fn sweep(phase: f32) -> (f32, f32, f32) {
    let (start, end) = if phase < 0.20 {
        (0.0, ease(phase / 0.20))
    } else if phase < 0.28 {
        (0.0, 1.0)
    } else if phase < 0.51 {
        (ease((phase - 0.28) / 0.23), 1.0)
    } else if phase < 0.58 {
        (1.0, 1.0)
    } else if phase < 0.82 {
        (1.0 - ease((phase - 0.58) / 0.24), 1.0)
    } else if phase < 0.83 {
        (0.0, 1.0)
    } else {
        (0.0, 1.0 - ease(((phase - 0.83) / 0.13).min(1.0)))
    };
    let opacity = if phase < 0.20 {
        0.3 + 0.7 * ease(phase / 0.20)
    } else {
        1.0 - 0.7 * ease((phase - 0.20) / 0.80)
    };
    (start, end, opacity)
}

// CSS ease-in-out is cubic-bezier(.42, 0, .58, 1). Solve its x coordinate for time.
fn ease(time: f32) -> f32 {
    let mut t = time;
    for _ in 0..3 {
        let x = t * (1.26 + t * (-0.78 + 0.52 * t));
        t -= (x - time) / (1.26 + t * (-1.56 + 1.56 * t));
    }
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, RawInput, Shape, vec2};

    #[test]
    fn overlay_moves_between_frames_without_resizing_the_placeholder() {
        let ctx = Context::default();
        let size = vec2(192.0, 24.0);
        let frame = |time, reduced_motion| {
            let mut rect = Rect::NOTHING;
            let output = ctx.run_ui(
                RawInput {
                    time: Some(time),
                    ..RawInput::default()
                },
                |ui| {
                    if reduced_motion {
                        ui.style_mut().animation_time = 0.0;
                    }
                    Skeleton::new(size).show(ui);
                    rect = ui.min_rect();
                },
            );
            let clips: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Rect(block) if block.rect.size() == size => Some(shape.clip_rect),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            (rect, clips)
        };
        let (first_rect, first) = frame(0.3, false);
        let (next_rect, next) = frame(1.2, false);
        assert_eq!(first_rect, next_rect);
        assert_eq!(first.len(), 2);
        assert_eq!(next.len(), 2);
        assert!(first[1].left() < next[1].left());
        assert!(first[1].right() < next[1].right());
        let (still_rect, still) = frame(1.8, true);
        assert_eq!(first_rect, still_rect);
        assert_eq!(still.len(), 1);
        assert_eq!(frame(2.4, true).1, still);
    }
}
