//! Interactive square image cropping.

use egui::{Pos2, Rect, Response, Sense, Ui, Vec2, emath::Numeric, load::SizedTexture};

const CROP_EDGE: f32 = 280.0;
const MAX_ZOOM: f32 = 4.0;

/// Pan and zoom retained by an image-crop editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    center: Pos2,
    zoom: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            center: Pos2::new(0.5, 0.5),
            zoom: 1.0,
        }
    }
}

impl State {
    #[must_use]
    pub const fn from_center_zoom(center: [f32; 2], zoom: f32) -> Self {
        Self {
            center: Pos2::new(center[0], center[1]),
            zoom,
        }
    }

    #[must_use]
    pub const fn center(&self) -> [f32; 2] {
        [self.center.x, self.center.y]
    }

    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.zoom
    }
}

/// Inputs other than the edited crop state.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub texture: SizedTexture,
    pub source_size: [u32; 2],
    pub zoom_label: &'a str,
    pub disabled: bool,
}

/// A square crop in source-image pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelCrop {
    left: u32,
    top: u32,
    edge: u32,
}

impl PixelCrop {
    #[must_use]
    pub const fn left(self) -> u32 {
        self.left
    }

    #[must_use]
    pub const fn top(self) -> u32 {
        self.top
    }

    #[must_use]
    pub const fn edge(self) -> u32 {
        self.edge
    }
}

/// Rendered crop and current source selection.
pub struct Output {
    pub response: Response,
    pub crop: PixelCrop,
}

#[must_use]
pub fn show(ui: &mut Ui, state: &mut State, props: Props<'_>) -> Output {
    let available = ui.available_width().clamp(1.0, CROP_EDGE);
    let crop_size = Vec2::splat(available);
    let (rect, mut response) = ui.allocate_exact_size(
        crop_size,
        if props.disabled {
            Sense::hover()
        } else {
            Sense::click_and_drag()
        },
    );
    let source_size = Vec2::new(
        f32::from_f64(props.source_size[0].to_f64()),
        f32::from_f64(props.source_size[1].to_f64()),
    );

    if !props.disabled && response.dragged() {
        let delta = ui.input(|input| input.pointer.delta());
        let rendered = rendered_size(source_size, rect.width(), state.zoom);
        state.center -= delta / rendered;
    }

    if !props.disabled && response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let previous = rendered_size(source_size, rect.width(), state.zoom);
            let pointer = response.hover_pos().unwrap_or_else(|| rect.center());
            let anchor = state.center + (pointer - rect.center()) / previous;
            state.zoom = (state.zoom * (scroll * 0.005).exp()).clamp(1.0, MAX_ZOOM);
            let rendered = rendered_size(source_size, rect.width(), state.zoom);
            state.center = anchor - (pointer - rect.center()) / rendered;
        }
    }

    constrain(state, source_size, rect.width());
    let uv = crop_uv(*state, source_size, rect.width());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        0.0,
        crate::theme::color32(palette.surfaces().field(garmin_color::theme::Level::Two)),
    );
    egui::Image::new(props.texture)
        .uv(uv)
        .fit_to_exact_size(rect.size())
        .corner_radius((rect.width() / 2.0).round())
        .paint_at(ui, rect);
    ui.painter().circle_stroke(
        rect.center(),
        rect.width() / 2.0 - 1.0,
        egui::Stroke::new(2.0, crate::theme::color32(palette.interaction().focus())),
    );

    if response.hovered() {
        let cursor = if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        };
        response = response.on_hover_cursor(cursor);
    }

    ui.add_space(12.0);
    let slider_width = (available - 96.0).clamp(80.0, 220.0);
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = slider_width;
        ui.horizontal(|ui| {
            ui.label(props.zoom_label);
            ui.add_enabled(
                !props.disabled,
                egui::Slider::new(&mut state.zoom, 1.0..=MAX_ZOOM).show_value(false),
            );
        });
    });
    constrain(state, source_size, rect.width());

    Output {
        response,
        crop: pixel_crop(
            crop_uv(*state, source_size, rect.width()),
            props.source_size,
        ),
    }
}

fn rendered_size(source: Vec2, crop_edge: f32, zoom: f32) -> Vec2 {
    source * (crop_edge / source.min_elem()) * zoom
}

fn crop_uv(state: State, source: Vec2, crop_edge: f32) -> Rect {
    let rendered = rendered_size(source, crop_edge, state.zoom);
    let half = Vec2::splat(crop_edge / 2.0) / rendered;
    Rect::from_min_max(state.center - half, state.center + half)
}

fn constrain(state: &mut State, source: Vec2, crop_edge: f32) {
    state.zoom = state.zoom.clamp(1.0, MAX_ZOOM);
    let rendered = rendered_size(source, crop_edge, state.zoom);
    let half = Vec2::splat(crop_edge / 2.0) / rendered;
    state.center.x = state.center.x.clamp(half.x, 1.0 - half.x);
    state.center.y = state.center.y.clamp(half.y, 1.0 - half.y);
}

fn pixel_crop(uv: Rect, source: [u32; 2]) -> PixelCrop {
    let width = f32::from_f64(source[0].to_f64());
    let height = f32::from_f64(source[1].to_f64());
    let edge = u32::from_f64(
        (uv.width() * width)
            .min(uv.height() * height)
            .round()
            .clamp(1.0, width.min(height))
            .to_f64(),
    );
    let max_left = f32::from_f64((source[0] - edge).to_f64());
    let max_top = f32::from_f64((source[1] - edge).to_f64());
    let left = u32::from_f64((uv.min.x * width).round().clamp(0.0, max_left).to_f64());
    let top = u32::from_f64((uv.min.y * height).round().clamp(0.0, max_top).to_f64());
    PixelCrop { left, top, edge }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_crop_covers_the_short_dimension() {
        let uv = crop_uv(State::default(), Vec2::new(800.0, 400.0), 280.0);
        assert_eq!(
            pixel_crop(uv, [800, 400]),
            PixelCrop {
                left: 200,
                top: 0,
                edge: 400
            }
        );
    }

    #[test]
    fn constrained_crop_never_exceeds_the_source() {
        let mut state = State {
            center: Pos2::new(-4.0, 7.0),
            zoom: MAX_ZOOM,
        };
        let source = Vec2::new(400.0, 800.0);
        constrain(&mut state, source, 280.0);
        let crop = pixel_crop(crop_uv(state, source, 280.0), [400, 800]);
        assert_eq!(
            crop,
            PixelCrop {
                left: 0,
                top: 700,
                edge: 100
            }
        );
    }
}
