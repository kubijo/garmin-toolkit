use std::io::Read;

use flate2::read::GzDecoder;

use super::{ActivityCase, EncodingError};

const HEADER: &str = "elapsed_seconds,latitude_degrees,longitude_degrees,elevation_meters,distance_meters,speed_meters_per_second,heart_rate_bpm,cadence_rpm,power_watts,temperature_celsius,lap_end";

pub(super) struct Activity {
    pub samples: Vec<Sample>,
}

impl Activity {
    pub fn duration_seconds(&self) -> u32 {
        self.samples
            .last()
            .map_or(0, |sample| sample.elapsed_seconds)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub elapsed_seconds: u32,
    pub latitude_degrees: Option<f64>,
    pub longitude_degrees: Option<f64>,
    pub elevation_meters: Option<f64>,
    pub distance_meters: Option<f64>,
    pub speed_meters_per_second: Option<f64>,
    pub heart_rate_bpm: Option<u8>,
    pub cadence_rpm: Option<u8>,
    pub power_watts: Option<u16>,
    pub temperature_celsius: Option<i8>,
    pub lap_end: bool,
}

pub(super) fn load(case: ActivityCase) -> Result<Activity, EncodingError> {
    let mut csv = String::new();
    GzDecoder::new(bytes(case))
        .read_to_string(&mut csv)
        .map_err(|error| invalid(case, format!("could not decompress recording: {error}")))?;
    let mut lines = csv.lines();
    if lines.next() != Some(HEADER) {
        return Err(invalid(case, "recording header does not match"));
    }

    let mut samples = Vec::new();
    for (line_index, line) in lines.enumerate() {
        samples.push(parse_sample(case, line_index + 2, line)?);
    }
    validate(case, &samples)?;
    Ok(Activity { samples })
}

fn parse_sample(
    case: ActivityCase,
    line_number: usize,
    line: &str,
) -> Result<Sample, EncodingError> {
    let fields = line.split(',').collect::<Vec<_>>();
    let [
        elapsed,
        latitude,
        longitude,
        elevation,
        distance,
        speed,
        heart_rate,
        cadence,
        power,
        temperature,
        lap_end,
    ] = fields.as_slice()
    else {
        return Err(invalid(
            case,
            format!("recording line {line_number} does not have eleven fields"),
        ));
    };
    Ok(Sample {
        elapsed_seconds: required(case, line_number, "elapsed seconds", elapsed)?,
        latitude_degrees: optional_f64(case, line_number, "latitude", latitude)?,
        longitude_degrees: optional_f64(case, line_number, "longitude", longitude)?,
        elevation_meters: optional_f64(case, line_number, "elevation", elevation)?,
        distance_meters: optional_f64(case, line_number, "distance", distance)?,
        speed_meters_per_second: optional_f64(case, line_number, "speed", speed)?,
        heart_rate_bpm: optional(case, line_number, "heart rate", heart_rate)?,
        cadence_rpm: optional(case, line_number, "cadence", cadence)?,
        power_watts: optional(case, line_number, "power", power)?,
        temperature_celsius: optional(case, line_number, "temperature", temperature)?,
        lap_end: match *lap_end {
            "0" => false,
            "1" => true,
            _ => {
                return Err(invalid(
                    case,
                    format!("invalid lap marker on line {line_number}"),
                ));
            }
        },
    })
}

fn required<T>(
    case: ActivityCase,
    line_number: usize,
    field: &'static str,
    value: &str,
) -> Result<T, EncodingError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse().map_err(|error| {
        invalid(
            case,
            format!("invalid {field} on line {line_number}: {error}"),
        )
    })
}

fn optional<T>(
    case: ActivityCase,
    line_number: usize,
    field: &'static str,
    value: &str,
) -> Result<Option<T>, EncodingError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    if value.is_empty() {
        Ok(None)
    } else {
        required(case, line_number, field, value).map(Some)
    }
}

fn optional_f64(
    case: ActivityCase,
    line_number: usize,
    field: &'static str,
    value: &str,
) -> Result<Option<f64>, EncodingError> {
    let parsed = optional(case, line_number, field, value)?;
    if parsed.is_some_and(|value: f64| !value.is_finite()) {
        Err(invalid(
            case,
            format!("non-finite {field} on line {line_number}"),
        ))
    } else {
        Ok(parsed)
    }
}

fn validate(case: ActivityCase, samples: &[Sample]) -> Result<(), EncodingError> {
    if samples.len() < 2 {
        return Err(invalid(case, "recording has fewer than two samples"));
    }
    if !samples
        .windows(2)
        .all(|pair| pair[0].elapsed_seconds < pair[1].elapsed_seconds)
    {
        return Err(invalid(
            case,
            "recording timestamps are not strictly ordered",
        ));
    }
    if samples.iter().any(|sample| {
        sample.latitude_degrees.is_some() != sample.longitude_degrees.is_some()
            || sample
                .latitude_degrees
                .is_some_and(|latitude| !(-90.0..=90.0).contains(&latitude))
            || sample
                .longitude_degrees
                .is_some_and(|longitude| !(-180.0..=180.0).contains(&longitude))
    }) {
        return Err(invalid(case, "recording has an invalid coordinate"));
    }
    if !samples.last().is_some_and(|sample| sample.lap_end) {
        return Err(invalid(case, "recording does not end its final lap"));
    }
    Ok(())
}

fn invalid(case: ActivityCase, reason: impl std::fmt::Display) -> EncodingError {
    EncodingError(format!("{}: {reason}", case.file_name()))
}

macro_rules! recording {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../infra/fixtures/fit/development-activities/recordings/",
            $name,
            ".csv.gz"
        ))
    };
}

const fn bytes(case: ActivityCase) -> &'static [u8] {
    match case {
        ActivityCase::ForestRun => recording!("forest-run"),
        ActivityCase::CoastalRun => recording!("coastal-run"),
        ActivityCase::CityRun => recording!("city-run"),
        ActivityCase::CityRide => recording!("city-ride"),
        ActivityCase::OpenWaterSwim => recording!("open-water-swim"),
        ActivityCase::IndoorPowerRide => recording!("indoor-power-ride"),
        ActivityCase::NeighborhoodWalk => recording!("neighborhood-walk"),
        ActivityCase::MountainHike => recording!("mountain-hike"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_approved_recording_is_ordered_and_complete() -> Result<(), EncodingError> {
        let expectations = [
            (ActivityCase::NeighborhoodWalk, 370, 3_107, 3_842.57),
            (ActivityCase::MountainHike, 2_271, 11_374, 12_571.48),
            (ActivityCase::ForestRun, 781, 3_931, 14_134.31),
            (ActivityCase::CoastalRun, 241, 1_185, 5_048.28),
            (ActivityCase::CityRun, 1_025, 5_234, 14_793.04),
            (ActivityCase::CityRide, 2_259, 11_963, 67_917.19),
            (ActivityCase::OpenWaterSwim, 420, 2_085, 1_620.54),
            (ActivityCase::IndoorPowerRide, 721, 3_600, 50_000.60),
        ];
        for (case, sample_count, duration, distance) in expectations {
            let activity = load(case)?;
            assert_eq!(activity.samples.len(), sample_count);
            assert_eq!(activity.duration_seconds(), duration);
            assert_eq!(
                activity
                    .samples
                    .last()
                    .and_then(|sample| sample.distance_meters),
                Some(distance)
            );
        }
        Ok(())
    }
}
