//! Geographic path previews.

use cint::ColorInterop;
use egui::{Align2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use garmin_color::theme;

use crate::theme::color32;

const DEFAULT_HEIGHT: f32 = 220.0;
const PADDING: f32 = 18.0;
const PATH_WIDTH: f32 = 3.0;
const ENDPOINT_RADIUS: f32 = 4.0;

/// One latitude/longitude pair in decimal degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub latitude: f64,
    pub longitude: f64,
}

/// One continuous path segment.
#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    pub points: &'a [Point],
}

/// Path-preview inputs.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub segments: &'a [Segment<'a>],
    pub empty: &'a str,
    pub height: Option<f32>,
}

/// Renders a fitted geographic path without a base map.
pub fn preview(ui: &mut Ui, props: &Props<'_>) {
    let height = props.height.unwrap_or(DEFAULT_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        0.0,
        palette.surfaces().layer(theme::Level::One).into_cint(),
    );

    let drawable = props
        .segments
        .iter()
        .filter(|segment| segment.points.len() >= 2)
        .collect::<Vec<_>>();
    let Some(bounds) = Bounds::from_segments(&drawable) else {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            props.empty,
            egui::TextStyle::Body.resolve(ui.style()),
            color32(palette.content().text_secondary()),
        );
        return;
    };

    let target = rect.shrink(PADDING);
    let stroke = Stroke::new(PATH_WIDTH, palette.interaction().interactive().into_cint());
    for segment in drawable {
        let points = segment
            .points
            .iter()
            .map(|point| bounds.project(*point, target))
            .collect::<Vec<_>>();
        if let Some(start) = points.first() {
            ui.painter().circle_filled(
                *start,
                ENDPOINT_RADIUS,
                palette.content().icon_primary().into_cint(),
            );
        }
        if let Some(end) = points.last() {
            ui.painter().circle_stroke(*end, ENDPOINT_RADIUS, stroke);
        }
        ui.painter().add(Shape::line(points, stroke));
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    minimum_x: f64,
    maximum_x: f64,
    minimum_y: f64,
    maximum_y: f64,
}

impl Bounds {
    fn from_segments(segments: &[&Segment<'_>]) -> Option<Self> {
        let mut points = segments.iter().flat_map(|segment| segment.points.iter());
        let first = Projected::from_point(*points.next()?);
        let mut bounds = Self {
            minimum_x: first.x,
            maximum_x: first.x,
            minimum_y: first.y,
            maximum_y: first.y,
        };
        for point in points.map(|point| Projected::from_point(*point)) {
            bounds.minimum_x = bounds.minimum_x.min(point.x);
            bounds.maximum_x = bounds.maximum_x.max(point.x);
            bounds.minimum_y = bounds.minimum_y.min(point.y);
            bounds.maximum_y = bounds.maximum_y.max(point.y);
        }
        Some(bounds)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "fitted viewport coordinates are intentionally narrowed to egui's f32 space"
    )]
    fn project(self, point: Point, target: Rect) -> egui::Pos2 {
        let point = Projected::from_point(point);
        let source_width = (self.maximum_x - self.minimum_x).max(f64::EPSILON);
        let source_height = (self.maximum_y - self.minimum_y).max(f64::EPSILON);
        let scale = (f64::from(target.width()) / source_width)
            .min(f64::from(target.height()) / source_height);
        let width = source_width * scale;
        let height = source_height * scale;
        let left = f64::from(target.center().x) - width / 2.0;
        let top = f64::from(target.center().y) - height / 2.0;
        egui::pos2(
            (point.x - self.minimum_x).mul_add(scale, left) as f32,
            (point.y - self.minimum_y).mul_add(scale, top) as f32,
        )
    }
}

#[derive(Clone, Copy)]
struct Projected {
    x: f64,
    y: f64,
}

impl Projected {
    fn from_point(point: Point) -> Self {
        const MAX_LATITUDE: f64 = 85.051_128_78;
        let latitude = point
            .latitude
            .clamp(-MAX_LATITUDE, MAX_LATITUDE)
            .to_radians();
        Self {
            x: point.longitude.to_radians(),
            y: -latitude.tan().asinh(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_preserves_geographic_orientation() {
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));
        let points = [
            Point {
                latitude: 50.0,
                longitude: 14.0,
            },
            Point {
                latitude: 51.0,
                longitude: 15.0,
            },
        ];
        let segment = Segment { points: &points };
        let bounds = Bounds::from_segments(&[&segment]).expect("the segment has two points");
        let southwest = bounds.project(points[0], rect);
        let northeast = bounds.project(points[1], rect);

        assert!(southwest.x < northeast.x);
        assert!(southwest.y > northeast.y);
    }
}
