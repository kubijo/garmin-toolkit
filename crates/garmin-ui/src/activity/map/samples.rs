//! Geometry-only map input shared by recorded activities and planned routes.

use garmin_model::{
    activity::Speed,
    route::{Coordinate, RoutePoint},
};
use garmin_service_api::ActivitySampleSnapshot;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct MapSample {
    pub coordinate: Option<Coordinate>,
    pub speed: Option<Speed>,
}

#[derive(Clone, Copy)]
pub(crate) enum Samples<'a> {
    Activity(&'a [ActivitySampleSnapshot]),
    Planned(&'a [RoutePoint]),
    Projected(&'a [MapSample]),
}

impl<'a> Samples<'a> {
    pub fn len(self) -> usize {
        match self {
            Self::Activity(values) => values.len(),
            Self::Planned(values) => values.len(),
            Self::Projected(values) => values.len(),
        }
    }

    pub fn identity(self) -> usize {
        match self {
            Self::Activity(values) => values.as_ptr() as usize,
            Self::Planned(values) => values.as_ptr() as usize,
            Self::Projected(values) => values.as_ptr() as usize,
        }
    }

    pub fn slice(self, range: std::ops::Range<usize>) -> Self {
        match self {
            Self::Activity(values) => Self::Activity(&values[range]),
            Self::Planned(values) => Self::Planned(&values[range]),
            Self::Projected(values) => Self::Projected(&values[range]),
        }
    }

    pub fn iter(self) -> impl DoubleEndedIterator<Item = MapSample> + ExactSizeIterator + 'a {
        (0..self.len()).map(move |index| match self {
            Self::Activity(values) => MapSample {
                coordinate: values[index].coordinate,
                speed: values[index].speed,
            },
            Self::Planned(values) => MapSample {
                coordinate: Some(values[index].coordinate()),
                speed: None,
            },
            Self::Projected(values) => values[index],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_model::route::{Latitude, Longitude};

    #[test]
    fn planned_geometry_preserves_coordinates_without_recorded_metrics() {
        let coordinate = Coordinate::from_parts(
            Latitude::from_degrees(50.1).unwrap(),
            Longitude::from_degrees(14.2).unwrap(),
        );
        let points = [RoutePoint::from_parts(coordinate, None)];
        let sample = Samples::Planned(&points).iter().next().unwrap();
        assert_eq!(sample.coordinate, Some(coordinate));
        assert!(sample.speed.is_none());
        let encoded = postcard::to_allocvec(&sample).unwrap();
        let decoded: MapSample = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(decoded.coordinate, sample.coordinate);
        assert!(decoded.speed.is_none());
    }
}
