//! Portable recorded activities: typed CSV, GPX, provenance, and authoritative FIT.

use std::io;
use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use datapackage::{
    Constraints, Descriptor, Field, FieldNames, ForeignKey, Metadata, Reference,
    ResourceDescriptor, TableSchema as Schema,
};

use garmin_model::{
    activity::{
        ActivityDuration, ActivityMetrics, ActivitySport, ActivitySummary, ActivityTotals, Cadence,
        Distance, Energy, HeartRate, Power, Speed, Temperature, TimeRange, TimerState,
    },
    artifact::{AcquisitionOperationId, ArtifactDigest, ByteCount, SourceIdentity},
    identity::{Source, SourceId},
    observation::ObservationId,
    route::{Coordinate, Elevation, Latitude, Longitude},
    value::Timestamp,
};
use garmin_service_api::DeviceFitPreviewActivity;
use garmin_storage::ActivityExport;
use serde::{Deserialize, Serialize};

use crate::devices::fit_preview::activity_snapshot;
use crate::{Application, FitImportRequest, FitImportResult, ImportDisposition, UserContext};

mod provenance;
mod read;
pub use provenance::{ActivityExportProvenance, ExportTransformation};
pub use read::{Package, Provenance as ParsedProvenance, read};

const MAX_OPEN_FIT_BYTES: ByteCount = ByteCount::from_mebibytes(64);

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
    /// Validates a Toolkit ZIP and imports its original FIT files into one profile.
    ///
    /// # Errors
    /// Returns validation or import failures.
    /// Retrying a partially imported package skips committed originals.
    pub async fn import_activity_package(
        &self,
        user: UserContext,
        archive: &Path,
    ) -> Result<usize, Error> {
        let preview = self.preview_activity_package(user, archive).await?;
        self.confirm_activity_package(preview).await
    }

    /// Validates all originals and prepares an import without writing to the profile.
    /// # Errors
    /// Returns an error for an unknown profile or invalid package.
    pub async fn preview_activity_package(
        &self,
        user: UserContext,
        archive: &Path,
    ) -> Result<PreparedActivityPackage, Error> {
        self.prepare_activity_import(user, ActivityImportPreview::from_package(archive)?)
            .await
    }

    /// Previews a FIT recording handed off by a desktop launcher.
    /// # Errors
    /// Returns an error for an unreadable or invalid recording or an unknown profile.
    pub async fn preview_activity_fit(
        &self,
        user: UserContext,
        path: &Path,
    ) -> Result<PreparedActivityPackage, Error> {
        self.prepare_activity_import(user, ActivityImportPreview::from_fit(path)?)
            .await
    }

    /// Checks the destination and binds the inspected originals to that profile.
    ///
    /// # Errors
    /// Returns an error if the destination is missing or its import history cannot be read.
    pub async fn prepare_activity_import(
        &self,
        user: UserContext,
        preview: ActivityImportPreview,
    ) -> Result<PreparedActivityPackage, Error> {
        if self.storage.user(user.user_id()).await?.is_none() {
            return Err(Error::Invalid(
                "The destination profile no longer exists".to_owned(),
            ));
        }
        let ActivityImportPreview {
            originals,
            namespace_name,
            label,
        } = preview;
        let namespace = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("garmin-toolkit/{namespace_name}/{}", user.user_id()).as_bytes(),
        );
        let source = Source::from_parts(
            SourceId::from_u128(namespace.as_u128()),
            user.user_id(),
            label
                .parse()
                .map_err(|error: garmin_model::identity::Error| {
                    Error::Invalid(error.to_string())
                })?,
            None,
        );
        let mut entries = Vec::with_capacity(originals.len());
        for InspectedOriginal {
            digest,
            bytes,
            activities,
        } in originals
        {
            let identity = SourceIdentity::from_string(format!("originals/{digest}.fit"))
                .map_err(|error| Error::Invalid(error.to_string()))?;
            let operation = uuid::Uuid::new_v5(&namespace, digest.as_bytes());
            let operation = AcquisitionOperationId::from_u128(operation.as_u128());
            let already_imported = self
                .fit_import_completed(user, operation)
                .await
                .map_err(|error| Error::Invalid(error.to_string()))?;
            entries.push(PackageOriginal {
                identity,
                operation,
                bytes,
                activities: activities
                    .into_iter()
                    .map(|activity| activity.summary)
                    .collect(),
                already_imported,
            });
        }
        Ok(PreparedActivityPackage {
            user,
            source,
            entries,
        })
    }

    /// Imports the exact originals validated for this destination profile.
    /// # Errors
    /// Returns an import error. Retrying skips commits from an interrupted import.
    pub async fn confirm_activity_package(
        &self,
        package: PreparedActivityPackage,
    ) -> Result<usize, Error> {
        let PreparedActivityPackage {
            user,
            source,
            entries,
        } = package;
        let mut imported = 0;
        for entry in entries {
            let result = self
                .import_fit(FitImportRequest::from_parts(
                    user,
                    &source,
                    entry.identity,
                    entry.operation,
                    Timestamp::from_jiff(jiff::Timestamp::now()),
                    &entry.bytes,
                ))
                .await
                .map_err(|error| Error::Invalid(error.to_string()))?;
            match result {
                FitImportResult::Imported {
                    observations,
                    disposition: ImportDisposition::Created,
                } => imported += observations.len(),
                FitImportResult::Imported { .. } => {}
                FitImportResult::Rejected { failure, .. } => {
                    return Err(Error::Invalid(failure.to_string()));
                }
            }
        }
        Ok(imported)
    }

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
        self.export_activity_with_limit(
            actor,
            selected,
            destination,
            ByteCount::from_bytes(u64::MAX),
        )
        .await
    }

    /// Exports an activity while bounding the size of the temporary ZIP.
    /// # Errors
    /// Returns export errors or an I/O error if the ZIP would exceed the limit.
    pub async fn export_activity_with_limit(
        &self,
        actor: UserContext,
        selected: ObservationId,
        destination: &Path,
        limit: ByteCount,
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
        let report = write_package(
            LimitedWriter {
                file: temporary.as_file_mut(),
                position: ByteCount::default(),
                limit,
            },
            selected,
            &recordings,
        )?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        Ok(Some(report))
    }
}

/// Inspected activity contents and the exact originals available for import.
pub struct ActivityImportPreview {
    originals: Vec<InspectedOriginal>,
    namespace_name: &'static str,
    label: &'static str,
}

struct InspectedOriginal {
    digest: String,
    bytes: Vec<u8>,
    activities: Vec<DeviceFitPreviewActivity>,
}

impl ActivityImportPreview {
    /// Reads and validates the originals from a Toolkit activity package.
    ///
    /// # Errors
    /// Returns an error for an unreadable or invalid package.
    pub fn from_package(path: &Path) -> Result<Self, Error> {
        let package = read(std::fs::File::open(path)?)?;
        Self::from_originals(
            package.originals.into_values(),
            "activity-package",
            "Toolkit activity package",
        )
    }

    /// Reads and validates a FIT recording.
    ///
    /// # Errors
    /// Returns an error for an unreadable or invalid recording.
    pub fn from_fit(path: &Path) -> Result<Self, Error> {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(Error::Invalid("Expected a regular FIT file".to_owned()));
        }
        let too_large = || Error::Invalid("FIT file exceeds the 64 MiB preview limit".to_owned());
        if metadata.len() > MAX_OPEN_FIT_BYTES.as_bytes() {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_OPEN_FIT_BYTES.as_bytes() + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_OPEN_FIT_BYTES.as_bytes() {
            return Err(too_large());
        }
        Self::from_originals([bytes], "open-fit", "Opened FIT recording")
    }

    fn from_originals(
        files: impl IntoIterator<Item = Vec<u8>>,
        namespace_name: &'static str,
        label: &'static str,
    ) -> Result<Self, Error> {
        let unique = files
            .into_iter()
            .map(|bytes| (ArtifactDigest::from_bytes(&bytes).to_string(), bytes))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut originals = Vec::with_capacity(unique.len());
        for (digest, bytes) in unique {
            let recorded = garmin_fit::normalize_activities(&bytes)
                .map_err(|error| Error::Invalid(error.to_string()))?;
            let activities = recorded
                .normalized_activities()
                .map(activity_snapshot)
                .collect();
            originals.push(InspectedOriginal {
                digest,
                bytes,
                activities,
            });
        }
        Ok(Self {
            originals,
            namespace_name,
            label,
        })
    }

    pub fn activities(&self) -> impl Iterator<Item = ActivitySummary> + '_ {
        self.recordings().map(|activity| activity.summary)
    }

    pub fn recordings(&self) -> impl Iterator<Item = &DeviceFitPreviewActivity> + '_ {
        self.originals
            .iter()
            .flat_map(|original| &original.activities)
    }
}

/// An import bound to the profile and exact bytes reviewed by the user.
pub struct PreparedActivityPackage {
    user: UserContext,
    source: Source,
    entries: Vec<PackageOriginal>,
}

struct PackageOriginal {
    identity: SourceIdentity,
    operation: AcquisitionOperationId,
    bytes: Vec<u8>,
    activities: Vec<garmin_model::activity::ActivitySummary>,
    already_imported: bool,
}

impl PreparedActivityPackage {
    #[must_use]
    pub const fn user(&self) -> UserContext {
        self.user
    }

    /// Activity rows and whether each original has already been imported.
    pub fn activities(
        &self,
    ) -> impl Iterator<Item = (garmin_model::activity::ActivitySummary, bool)> + '_ {
        self.entries.iter().flat_map(|entry| {
            entry
                .activities
                .iter()
                .copied()
                .map(|activity| (activity, entry.already_imported))
        })
    }
}

struct LimitedWriter<'a> {
    file: &'a mut std::fs::File,
    position: ByteCount,
    limit: ByteCount,
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .position
            .checked_add(ByteCount::from_bytes(bytes.len() as u64))
            .is_none_or(|end| end > self.limit)
        {
            return Err(io::Error::other(
                "Activity export exceeds the download size limit",
            ));
        }
        let written = self.file.write(bytes)?;
        self.position = self
            .position
            .checked_add(ByteCount::from_bytes(written as u64))
            .expect("a successful write stays within the checked byte limit");
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for LimitedWriter<'_> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let position = self.file.seek(target)?;
        self.position = ByteCount::from_bytes(position);
        Ok(position)
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
impl Scalar for i64 {
    const KIND: &'static str = "integer";
}
impl Scalar for usize {
    const KIND: &'static str = "integer";
}
macro_rules! scalar_quantities {
    ($kind:literal: $($quantity:ty),+ $(,)?) => {
        $(impl Scalar for $quantity { const KIND: &'static str = $kind; })+
    };
}
scalar_quantities!("integer": ActivityDuration, Distance, Energy, HeartRate, Power, Speed, Temperature);
scalar_quantities!("number": Cadence, Elevation, Latitude, Longitude);
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
    elapsed_ms: ActivityDuration => "ms",
    timer_ms: ActivityDuration => "ms",
    distance_mm: Option<Distance> => "mm",
    energy_kcal: Option<Energy> => "kcal",
    ascent_mm: Option<Distance> => "mm",
    descent_mm: Option<Distance> => "mm",
    average_speed_mm_s: Option<Speed> => "mm/s",
    maximum_speed_mm_s: Option<Speed> => "mm/s",
    average_heart_rate_bpm: Option<HeartRate> => "beats/min",
    maximum_heart_rate_bpm: Option<HeartRate> => "beats/min",
    average_cadence_rpm: Option<Cadence> => "rev/min",
    maximum_cadence_rpm: Option<Cadence> => "rev/min",
    average_power_w: Option<Power> => "W",
    maximum_power_w: Option<Power> => "W",
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
            elapsed_ms: totals.elapsed(),
            timer_ms: totals.timer(),
            distance_mm: totals.distance(),
            energy_kcal: totals.energy(),
            ascent_mm: totals.ascent(),
            descent_mm: totals.descent(),
            average_speed_mm_s: metrics.average_speed(),
            maximum_speed_mm_s: metrics.maximum_speed(),
            average_heart_rate_bpm: metrics.average_heart_rate(),
            maximum_heart_rate_bpm: metrics.maximum_heart_rate(),
            average_cadence_rpm: metrics.average_cadence(),
            maximum_cadence_rpm: metrics.maximum_cadence(),
            average_power_w: metrics.average_power(),
            maximum_power_w: metrics.maximum_power(),
        }
    }
}

table!(Sample {
    observation_id: ObservationId => "",
    position: usize => "",
    timestamp_ms: i64 => "unix millisecond",
    latitude_degrees: Option<Latitude> => "degree",
    longitude_degrees: Option<Longitude> => "degree",
    elevation_m: Option<Elevation> => "m",
    distance_mm: Option<Distance> => "mm",
    speed_mm_s: Option<Speed> => "mm/s",
    heart_rate_bpm: Option<HeartRate> => "beats/min",
    cadence_rpm: Option<Cadence> => "rev/min",
    power_w: Option<Power> => "W",
    temperature_millicelsius: Option<Temperature> => "milli degree Celsius",
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
                latitude_degrees: point.coordinate().as_ref().map(Coordinate::latitude),
                longitude_degrees: point.coordinate().as_ref().map(Coordinate::longitude),
                elevation_m: point.elevation(),
                distance_mm: point.distance(),
                speed_mm_s: metrics.speed(),
                heart_rate_bpm: metrics.heart_rate(),
                cadence_rpm: metrics.cadence(),
                power_w: metrics.power(),
                temperature_millicelsius: metrics.temperature(),
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
    file: LimitedWriter<'_>,
    selected: ObservationId,
    recordings: &[ActivityExport],
) -> Result<Report, Error> {
    let mut writer = datapackage::Writer::new(file);
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
    writer: &mut datapackage::Writer<LimitedWriter<'_>>,
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
    writer: &mut datapackage::Writer<LimitedWriter<'_>>,
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
