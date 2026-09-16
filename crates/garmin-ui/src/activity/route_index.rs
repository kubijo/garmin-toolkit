//! Persistent world-space route index used by map pointer interaction.

use std::cmp::Ordering;

use garmin_service_api::ActivitySampleSnapshot;

const LEAF_SEGMENTS: usize = 12;
const MAX_MERCATOR_LATITUDE: f64 = 85.051_128_78;

#[derive(Default)]
pub(super) struct RouteIndex {
    origin_x: f64,
    segments: Vec<Segment>,
    root: Option<Node>,
}

impl RouteIndex {
    pub(super) fn new(samples: &[ActivitySampleSnapshot], sample_offset: usize) -> Self {
        let mut points = Vec::with_capacity(samples.len());
        let mut previous_longitude: Option<f64> = None;
        let mut unwrapped_longitude = 0.0;
        for (offset, sample) in samples.iter().enumerate() {
            let Some(coordinate) = sample.coordinate else {
                points.push(None);
                continue;
            };
            let longitude = coordinate.longitude().as_degrees();
            unwrapped_longitude = previous_longitude.map_or(longitude, |previous| {
                unwrapped_longitude + (longitude - previous + 540.0).rem_euclid(360.0) - 180.0
            });
            previous_longitude = Some(longitude);
            points.push(Some((
                sample_offset + offset,
                [
                    unwrapped_longitude / 360.0 + 0.5,
                    mercator_y(coordinate.latitude().as_degrees()),
                ],
            )));
        }
        Self::from_points(&points)
    }

    fn from_points(points: &[Option<(usize, [f64; 2])>]) -> Self {
        let segments = points
            .windows(2)
            .filter_map(|pair| match (pair[0], pair[1]) {
                (Some(start), Some(end)) => Some(Segment::new(start, end)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let origin_x = points
            .iter()
            .flatten()
            .next()
            .map_or(0.5, |point| point.1[0]);
        let root = (!segments.is_empty()).then(|| {
            let indices = (0..segments.len()).collect::<Vec<_>>();
            Node::build(&segments, indices)
        });
        Self {
            origin_x,
            segments,
            root,
        }
    }

    /// Return the original sample index nearest to a world-space pointer.
    pub(super) fn query(
        &self,
        mut pointer: [f64; 2],
        world_pixels: f64,
        radius_pixels: f64,
    ) -> Option<usize> {
        let root = self.root.as_ref()?;
        if !world_pixels.is_finite() || world_pixels <= 0.0 {
            return None;
        }
        pointer[0] += (self.origin_x - pointer[0]).round();
        let radius = radius_pixels / world_pixels;
        let query = Bounds::around(pointer, radius);
        let mut nearest = Nearest {
            distance_squared: radius * radius,
            sample: None,
        };
        root.query(&self.segments, query, pointer, &mut nearest);
        nearest.sample
    }
}

#[derive(Clone, Copy)]
struct Segment {
    start: [f64; 2],
    end: [f64; 2],
    start_index: usize,
    end_index: usize,
    bounds: Bounds,
}

impl Segment {
    fn new(start: (usize, [f64; 2]), end: (usize, [f64; 2])) -> Self {
        Self {
            start: start.1,
            end: end.1,
            start_index: start.0,
            end_index: end.0,
            bounds: Bounds::from_points(start.1, end.1),
        }
    }

    fn nearest(self, pointer: [f64; 2]) -> (usize, f64) {
        let delta = [self.end[0] - self.start[0], self.end[1] - self.start[1]];
        let length_squared = delta[0].mul_add(delta[0], delta[1] * delta[1]);
        let fraction = if length_squared > 0.0 {
            ((pointer[0] - self.start[0]) * delta[0] + (pointer[1] - self.start[1]) * delta[1])
                / length_squared
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let projected = [
            delta[0].mul_add(fraction, self.start[0]),
            delta[1].mul_add(fraction, self.start[1]),
        ];
        let distance = [pointer[0] - projected[0], pointer[1] - projected[1]];
        let index = if fraction < 0.5 {
            self.start_index
        } else {
            self.end_index
        };
        (
            index,
            distance[0].mul_add(distance[0], distance[1] * distance[1]),
        )
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    minimum: [f64; 2],
    maximum: [f64; 2],
}

impl Bounds {
    fn from_points(first: [f64; 2], second: [f64; 2]) -> Self {
        Self {
            minimum: [first[0].min(second[0]), first[1].min(second[1])],
            maximum: [first[0].max(second[0]), first[1].max(second[1])],
        }
    }

    fn around(point: [f64; 2], radius: f64) -> Self {
        Self {
            minimum: [point[0] - radius, point[1] - radius],
            maximum: [point[0] + radius, point[1] + radius],
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            minimum: [
                self.minimum[0].min(other.minimum[0]),
                self.minimum[1].min(other.minimum[1]),
            ],
            maximum: [
                self.maximum[0].max(other.maximum[0]),
                self.maximum[1].max(other.maximum[1]),
            ],
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.minimum[0] <= other.maximum[0]
            && self.maximum[0] >= other.minimum[0]
            && self.minimum[1] <= other.maximum[1]
            && self.maximum[1] >= other.minimum[1]
    }

    fn center(self, axis: usize) -> f64 {
        f64::midpoint(self.minimum[axis], self.maximum[axis])
    }

    fn span(self, axis: usize) -> f64 {
        self.maximum[axis] - self.minimum[axis]
    }
}

struct Node {
    bounds: Bounds,
    kind: NodeKind,
}

enum NodeKind {
    Leaf(Vec<usize>),
    Branch(Box<[Node; 2]>),
}

impl Node {
    fn build(segments: &[Segment], mut indices: Vec<usize>) -> Self {
        let bounds = indices
            .iter()
            .map(|index| segments[*index].bounds)
            .reduce(Bounds::union)
            .expect("a route index node contains at least one segment");
        if indices.len() <= LEAF_SEGMENTS {
            return Self {
                bounds,
                kind: NodeKind::Leaf(indices),
            };
        }
        let axis = usize::from(bounds.span(1) > bounds.span(0));
        indices.sort_unstable_by(|first, second| {
            segments[*first]
                .bounds
                .center(axis)
                .partial_cmp(&segments[*second].bounds.center(axis))
                .unwrap_or(Ordering::Equal)
        });
        let right = indices.split_off(indices.len() / 2);
        Self {
            bounds,
            kind: NodeKind::Branch(Box::new([
                Self::build(segments, indices),
                Self::build(segments, right),
            ])),
        }
    }

    fn query(&self, segments: &[Segment], query: Bounds, pointer: [f64; 2], nearest: &mut Nearest) {
        if !self.bounds.intersects(query) {
            return;
        }
        match &self.kind {
            NodeKind::Leaf(indices) => {
                for index in indices {
                    let segment = segments[*index];
                    if !segment.bounds.intersects(query) {
                        continue;
                    }
                    let (sample, distance_squared) = segment.nearest(pointer);
                    if distance_squared < nearest.distance_squared {
                        nearest.distance_squared = distance_squared;
                        nearest.sample = Some(sample);
                    }
                }
            }
            NodeKind::Branch(children) => {
                children[0].query(segments, query, pointer, nearest);
                children[1].query(segments, query, pointer, nearest);
            }
        }
    }
}

struct Nearest {
    distance_squared: f64,
    sample: Option<usize>,
}

fn mercator_y(latitude: f64) -> f64 {
    let latitude = latitude
        .clamp(-MAX_MERCATOR_LATITUDE, MAX_MERCATOR_LATITUDE)
        .to_radians();
    (1.0 - latitude.tan().asinh() / std::f64::consts::PI) / 2.0
}

#[cfg(test)]
mod tests {
    use super::RouteIndex;

    #[test]
    fn query_returns_the_nearest_original_endpoint() {
        let index = RouteIndex::from_points(&[
            Some((4, [0.10, 0.20])),
            Some((5, [0.20, 0.20])),
            Some((9, [0.30, 0.20])),
        ]);

        assert_eq!(index.query([0.27, 0.205], 1_000.0, 10.0), Some(9));
    }

    #[test]
    fn coordinate_gaps_do_not_create_phantom_segments() {
        let index =
            RouteIndex::from_points(&[Some((0, [0.10, 0.20])), None, Some((2, [0.30, 0.20]))]);

        assert_eq!(index.query([0.20, 0.20], 1_000.0, 20.0), None);
    }

    #[test]
    fn query_wraps_the_pointer_to_the_route_copy() {
        let index = RouteIndex::from_points(&[Some((0, [1.001, 0.50])), Some((1, [1.004, 0.50]))]);

        assert_eq!(index.query([0.003, 0.50], 10_000.0, 20.0), Some(1));
    }

    #[test]
    fn distant_pointer_is_rejected() {
        let index = RouteIndex::from_points(&[Some((0, [0.10, 0.20])), Some((1, [0.20, 0.20]))]);

        assert_eq!(index.query([0.15, 0.30], 1_000.0, 10.0), None);
    }
}
