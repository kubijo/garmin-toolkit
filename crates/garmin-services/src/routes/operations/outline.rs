use garmin_model::route::RoutePoint;
use garmin_service_api::routes::OutlinePoint;

const MAX_POINTS: usize = 64;

pub(super) fn sample(points: &[RoutePoint]) -> Vec<OutlinePoint> {
    if points.is_empty() {
        return Vec::new();
    }
    let count = points.len().min(MAX_POINTS);
    let mut selected = Vec::with_capacity(count);
    let mut next = 0;
    let mut previous = 0.0;
    let mut unwrapped = 0.0;
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for (index, point) in points.iter().enumerate() {
        let coordinate = point.coordinate();
        let longitude = coordinate.longitude().as_degrees();
        let latitude = coordinate.latitude().as_degrees();
        if index == 0 {
            unwrapped = longitude;
        } else {
            unwrapped += (longitude - previous + 180.0).rem_euclid(360.0) - 180.0;
        }
        previous = longitude;
        min_x = min_x.min(unwrapped);
        max_x = max_x.max(unwrapped);
        min_y = min_y.min(latitude);
        max_y = max_y.max(latitude);
        if index == next {
            selected.push((unwrapped, latitude));
            if selected.len() < count {
                next = selected.len() * (points.len() - 1) / (count - 1);
            }
        }
    }
    let longitude_scale = f64::midpoint(min_y, max_y)
        .to_radians()
        .cos()
        .abs()
        .max(0.01);
    let width = (max_x - min_x) * longitude_scale;
    let height = max_y - min_y;
    let scale = if width.max(height) > 0.0 {
        255.0 / width.max(height)
    } else {
        0.0
    };
    let left = (255.0 - width * scale) * 0.5;
    let top = (255.0 - height * scale) * 0.5;
    selected
        .into_iter()
        .map(|(x, y)| OutlinePoint {
            x: quantize(left + (x - min_x) * longitude_scale * scale),
            y: quantize(top + (max_y - y) * scale),
        })
        .collect()
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded coordinates are clamped to the u8 range"
)]
fn quantize(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_model::route::{Coordinate, Latitude, Longitude};

    fn point(latitude: f64, longitude: f64) -> RoutePoint {
        RoutePoint::from_parts(
            Coordinate::from_parts(
                Latitude::from_degrees(latitude).expect("valid latitude"),
                Longitude::from_degrees(longitude).expect("valid longitude"),
            ),
            None,
        )
    }

    #[test]
    fn outline_is_bounded_and_includes_both_ends() {
        let points: Vec<_> = (0..1000)
            .map(|index| point(10.0 + f64::from(index) / 1000.0, 20.0))
            .collect();
        let outline = sample(&points);
        assert_eq!(outline.len(), MAX_POINTS);
        assert_eq!(outline.first().map(|point| point.y), Some(255));
        assert_eq!(outline.last().map(|point| point.y), Some(0));
    }

    #[test]
    fn dateline_crossing_stays_local() {
        let outline = sample(&[point(0.0, 179.9), point(0.0, -179.9)]);
        assert_eq!(outline.first().map(|point| point.x), Some(0));
        assert_eq!(outline.last().map(|point| point.x), Some(255));
        assert!(outline.iter().all(|point| point.y == 128));
    }
}
