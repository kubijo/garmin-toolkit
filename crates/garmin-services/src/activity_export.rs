//! Portable recorded activities: typed CSV, GPX, provenance, and authoritative FIT.

use std::{collections::BTreeSet, path::Path};

use datapackage::{
    Constraints, Descriptor, Field, FieldNames, ForeignKey, Metadata, Reference,
    ResourceDescriptor, TableSchema as Schema,
};

use garmin_model::{
    activity::{ActivityMetrics, ActivitySport, ActivityTotals, TimeRange, TimerState},
    observation::ObservationId,
};
use garmin_storage::ActivityExport;
use serde::{Deserialize, Serialize};

use crate::{Application, UserContext};

mod provenance;
mod read;
pub use provenance::{ActivityExportProvenance, ExportTransformation};
pub use read::{Package, Provenance as ParsedProvenance, read};

/// Counts of preserved observations and original FIT artifacts.
#[derive(Debug, Eq, PartialEq)]
pub struct Report {
    pub observations: usize,
    pub originals: usize,
}

/// Export failures never replace an existing destination.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid activity package: {0}")]
    Invalid(String),
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Package(#[from] datapackage::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Gpx(#[from] garmin_gpx::Error),
}

impl Application {
    /// Exports one owned activity and associated recordings into a new ZIP Data Package.
    ///
    /// Returns `None` for an absent activity or another profile's activity.
    /// The original FIT files remain authoritative; CSV and GPX are normalized projections.
    ///
    /// # Errors
    /// [`enum@Error`] for invalid source data, serialization, or an existing/unwritable destination.
    pub async fn export_activity(
        &self,
        actor: UserContext,
        selected: ObservationId,
        destination: &Path,
    ) -> Result<Option<Report>, Error> {
        let recordings = self
            .storage
            .activity_export(actor.user_id(), selected)
            .await?;
        if recordings.is_empty() {
            return Ok(None);
        }
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let report = write_package(&mut temporary, selected, &recordings)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        Ok(Some(report))
    }
}

trait Scalar {
    const KIND: &'static str;
    const REQUIRED: bool = true;
}
impl Scalar for String {
    const KIND: &'static str = "string";
}
impl Scalar for ObservationId {
    const KIND: &'static str = "string";
}
impl Scalar for u64 {
    const KIND: &'static str = "integer";
}
impl Scalar for u32 {
    const KIND: &'static str = "integer";
}
impl Scalar for u16 {
    const KIND: &'static str = "integer";
}
impl Scalar for i32 {
    const KIND: &'static str = "integer";
}
impl Scalar for i64 {
    const KIND: &'static str = "integer";
}
impl Scalar for usize {
    const KIND: &'static str = "integer";
}
impl Scalar for f64 {
    const KIND: &'static str = "number";
}
impl<T: Scalar> Scalar for Option<T> {
    const KIND: &'static str = T::KIND;
    const REQUIRED: bool = false;
}

trait Table: Serialize {
    fn fields() -> Vec<Field>;
}

// CSV column names, scalar types, nullability,
// and schema metadata share one declaration.
macro_rules! table {
    ($name:ident { $($field:ident: $ty:ty => $unit:literal),* $(,)? }) => {
        #[derive(Debug, Deserialize, Serialize)]
        pub struct $name { $(pub $field: $ty),* }
        impl Table for $name {
            fn fields() -> Vec<Field> {
                vec![$(Field { name: stringify!($field).to_owned(), kind: <$ty as Scalar>::KIND.to_owned(), unit: if $unit.is_empty() { None } else { Some($unit.to_owned()) }, constraints: Constraints { required: <$ty as Scalar>::REQUIRED, ..Constraints::default() }, metadata: Metadata::new() }),*]
            }
        }
    };
}

table!(Aggregate {
    observation_id: ObservationId => "",
    position: usize => "",
    sport: String => "",
    start_ms: i64 => "unix millisecond",
    end_ms: i64 => "unix millisecond",
    elapsed_ms: u64 => "ms",
    timer_ms: u64 => "ms",
    distance_mm: Option<u64> => "mm",
    energy_kcal: Option<u32> => "kcal",
    ascent_mm: Option<u64> => "mm",
    descent_mm: Option<u64> => "mm",
    average_speed_mm_s: Option<u32> => "mm/s",
    maximum_speed_mm_s: Option<u32> => "mm/s",
    average_heart_rate_bpm: Option<u16> => "beats/min",
    maximum_heart_rate_bpm: Option<u16> => "beats/min",
    average_cadence_rpm: Option<f64> => "rev/min",
    maximum_cadence_rpm: Option<f64> => "rev/min",
    average_power_w: Option<u32> => "W",
    maximum_power_w: Option<u32> => "W",
});

impl Aggregate {
    fn new(
        observation_id: ObservationId,
        position: usize,
        sport: ActivitySport,
        time: TimeRange,
        totals: ActivityTotals,
        metrics: ActivityMetrics,
    ) -> Self {
        Self {
            observation_id,
            position,
            sport: match sport {
                ActivitySport::Running => "running",
                ActivitySport::Cycling => "cycling",
                ActivitySport::Swimming => "swimming",
                ActivitySport::Walking => "walking",
                ActivitySport::Hiking => "hiking",
            }
            .to_owned(),
            start_ms: time.start().as_unix_milliseconds(),
            end_ms: time.end().as_unix_milliseconds(),
            elapsed_ms: totals.elapsed().as_milliseconds(),
            timer_ms: totals.timer().as_milliseconds(),
            distance_mm: totals.distance().map(|v| v.as_millimeters()),
            energy_kcal: totals.energy().map(|v| v.as_kilocalories()),
            ascent_mm: totals.ascent().map(|v| v.as_millimeters()),
            descent_mm: totals.descent().map(|v| v.as_millimeters()),
            average_speed_mm_s: metrics
                .average_speed()
                .map(|v| v.as_millimeters_per_second()),
            maximum_speed_mm_s: metrics
                .maximum_speed()
                .map(|v| v.as_millimeters_per_second()),
            average_heart_rate_bpm: metrics
                .average_heart_rate()
                .map(|v| v.as_beats_per_minute()),
            maximum_heart_rate_bpm: metrics
                .maximum_heart_rate()
                .map(|v| v.as_beats_per_minute()),
            average_cadence_rpm: metrics
                .average_cadence()
                .map(|v| v.as_revolutions_per_minute()),
            maximum_cadence_rpm: metrics
                .maximum_cadence()
                .map(|v| v.as_revolutions_per_minute()),
            average_power_w: metrics.average_power().map(|v| v.as_watts()),
            maximum_power_w: metrics.maximum_power().map(|v| v.as_watts()),
        }
    }
}

table!(Sample {
    observation_id: ObservationId => "",
    position: usize => "",
    timestamp_ms: i64 => "unix millisecond",
    latitude_degrees: Option<f64> => "degree",
    longitude_degrees: Option<f64> => "degree",
    elevation_m: Option<f64> => "m",
    distance_mm: Option<u64> => "mm",
    speed_mm_s: Option<u32> => "mm/s",
    heart_rate_bpm: Option<u16> => "beats/min",
    cadence_rpm: Option<f64> => "rev/min",
    power_w: Option<u32> => "W",
    temperature_millicelsius: Option<i32> => "milli degree Celsius",
});
table!(Event { observation_id: ObservationId => "", position: usize => "", timestamp_ms: i64 => "unix millisecond", state: String => "" });

#[derive(Debug, Deserialize, Serialize)]
pub struct Creator {
    manufacturer: u16,
    product: Option<u16>,
    serial_number: Option<u32>,
    product_name: Option<String>,
    software_version_hundredths: Option<u16>,
}

impl From<&garmin_fit::CreatorDiagnostics> for Creator {
    fn from(value: &garmin_fit::CreatorDiagnostics) -> Self {
        Self {
            manufacturer: value.manufacturer().as_u16(),
            product: value.product().map(|v| v.as_u16()),
            serial_number: value.serial_number().map(|v| v.as_u32()),
            product_name: value.product_name().map(|v| v.as_str().to_owned()),
            software_version_hundredths: value.software_version().map(|v| v.as_hundredths()),
        }
    }
}

#[derive(Default)]
struct Rows {
    activities: Vec<Aggregate>,
    laps: Vec<Aggregate>,
    samples: Vec<Sample>,
    events: Vec<Event>,
}

impl Rows {
    fn push(&mut self, recording: &ActivityExport) {
        let id = recording.activity.observation_id();
        let activity = recording.activity.normalized().activity();
        let summary = activity.summary();
        self.activities.push(Aggregate::new(
            id,
            0,
            summary.sport(),
            summary.time(),
            summary.totals(),
            summary.metrics(),
        ));
        self.laps
            .extend(activity.laps().iter().enumerate().map(|(position, lap)| {
                Aggregate::new(
                    id,
                    position,
                    summary.sport(),
                    lap.time(),
                    lap.totals(),
                    lap.metrics(),
                )
            }));
        for (position, point) in activity.track().iter().enumerate() {
            let metrics = point.measurements();
            self.samples.push(Sample {
                observation_id: id,
                position,
                timestamp_ms: point.timestamp().as_unix_milliseconds(),
                latitude_degrees: point.coordinate().map(|v| v.latitude().as_degrees()),
                longitude_degrees: point.coordinate().map(|v| v.longitude().as_degrees()),
                elevation_m: point.elevation().map(|v| v.as_meters()),
                distance_mm: point.distance().map(|v| v.as_millimeters()),
                speed_mm_s: metrics.speed().map(|v| v.as_millimeters_per_second()),
                heart_rate_bpm: metrics.heart_rate().map(|v| v.as_beats_per_minute()),
                cadence_rpm: metrics.cadence().map(|v| v.as_revolutions_per_minute()),
                power_w: metrics.power().map(|v| v.as_watts()),
                temperature_millicelsius: metrics.temperature().map(|v| v.as_millicelsius()),
            });
        }
        self.events.extend(
            activity
                .timer_events()
                .iter()
                .enumerate()
                .map(|(position, event)| Event {
                    observation_id: id,
                    position,
                    timestamp_ms: event.timestamp().as_unix_milliseconds(),
                    state: match event.state() {
                        TimerState::Running => "running",
                        TimerState::Stopped => "stopped",
                    }
                    .to_owned(),
                }),
        );
    }
}

fn write_package(
    file: &mut tempfile::NamedTempFile,
    selected: ObservationId,
    recordings: &[ActivityExport],
) -> Result<Report, Error> {
    let mut writer = datapackage::Writer::new(file.as_file_mut());
    let mut rows = Rows::default();
    let mut originals = BTreeSet::new();
    for recording in recordings {
        rows.push(recording);
        let id = recording.activity.observation_id();
        let gpx = garmin_gpx::activity::write(recording.activity.normalized().activity())?;
        add_resource(
            &mut writer,
            format!("track-{id}"),
            format!("tracks/{id}.gpx"),
            "gpx",
            "application/gpx+xml",
            &gpx,
            None,
        )?;
        let artifact = recording.artifact.id();
        if originals.insert(artifact) {
            add_resource(
                &mut writer,
                format!("original-{artifact}"),
                format!("originals/{artifact}.fit"),
                "fit",
                "application/vnd.ant.fit",
                &recording.original,
                None,
            )?;
        }
    }
    add_table(
        &mut writer,
        "activities",
        &rows.activities,
        vec!["observation_id"],
        false,
    )?;
    add_table(
        &mut writer,
        "laps",
        &rows.laps,
        vec!["observation_id", "position"],
        true,
    )?;
    add_table(
        &mut writer,
        "samples",
        &rows.samples,
        vec!["observation_id", "position"],
        true,
    )?;
    add_table(
        &mut writer,
        "timer-events",
        &rows.events,
        vec!["observation_id", "position"],
        true,
    )?;
    let provenance = recordings
        .iter()
        .map(ParsedProvenance::try_from)
        .collect::<Result<Vec<_>, Error>>()?;
    add_resource(
        &mut writer,
        "provenance".to_owned(),
        "provenance.json".to_owned(),
        "json",
        "application/json",
        &serde_json::to_vec_pretty(&provenance)?,
        None,
    )?;
    writer.finish(Descriptor {
        schema: Some(datapackage::V2.to_owned()),
        name: Some("garmin-toolkit-activity".to_owned()),
        version: Some("0.1.0".to_owned()),
        metadata: Metadata::from([
            (
                "title".to_owned(),
                "Garmin Toolkit recorded activity".into(),
            ),
            (
                "selectedObservationId".to_owned(),
                serde_json::to_value(selected)?,
            ),
            ("provenance".to_owned(), "provenance.json".into()),
        ]),
        ..Descriptor::default()
    })?;
    Ok(Report {
        observations: recordings.len(),
        originals: originals.len(),
    })
}

fn add_table<T: Table>(
    writer: &mut datapackage::Writer<&mut std::fs::File>,
    name: &'static str,
    rows: &[T],
    primary_key: Vec<&'static str>,
    references_activity: bool,
) -> Result<(), Error> {
    let foreign_keys = if references_activity {
        vec![ForeignKey {
            fields: FieldNames::Many(vec!["observation_id".to_owned()]),
            reference: Reference {
                resource: "activities".to_owned(),
                fields: FieldNames::Many(vec!["observation_id".to_owned()]),
            },
        }]
    } else {
        Vec::new()
    };
    let schema = Schema {
        fields: T::fields(),
        primary_key: FieldNames::Many(primary_key.into_iter().map(str::to_owned).collect()),
        foreign_keys,
        missing_values: vec![String::new()],
        metadata: Metadata::new(),
    };
    let bytes = datapackage::csv_bytes(rows, &schema)?;
    add_resource(
        writer,
        name.to_owned(),
        format!("data/{name}.csv"),
        "csv",
        "text/csv",
        &bytes,
        Some(schema),
    )
}

fn add_resource(
    writer: &mut datapackage::Writer<&mut std::fs::File>,
    name: String,
    path: String,
    format: &'static str,
    mediatype: &'static str,
    bytes: &[u8],
    schema: Option<Schema>,
) -> Result<(), Error> {
    writer.add(
        ResourceDescriptor {
            kind: schema.as_ref().map(|_| "table".to_owned()),
            name,
            path: Some(path),
            format: Some(format.to_owned()),
            mediatype: Some(mediatype.to_owned()),
            schema,
            ..ResourceDescriptor::default()
        },
        bytes,
    )?;
    Ok(())
}
