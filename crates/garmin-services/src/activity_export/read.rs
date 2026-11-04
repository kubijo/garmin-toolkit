//! Bounded, offline parsing of Garmin Toolkit's activity Data Package profile.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek},
    path::Path,
};

use garmin_model::{
    artifact::{ArtifactDigest, ArtifactId},
    observation::ObservationId,
};
use garmin_storage::{ActivityExportProvenance, ExportTransformation};
use serde::{Deserialize, de::DeserializeOwned};

use super::{Aggregate, Creator, Error, Event, Sample, Table};

/// Parsed normalized relations and immutable original data.
/// Parsing does not import data.
pub struct Package {
    pub selected: ObservationId,
    pub activities: Vec<Aggregate>,
    pub laps: Vec<Aggregate>,
    pub samples: Vec<Sample>,
    pub timer_events: Vec<Event>,
    pub provenance: Vec<Provenance>,
    pub originals: BTreeMap<ArtifactId, Vec<u8>>,
    pub tracks: BTreeMap<ObservationId, Vec<u8>>,
}

/// Acquisition and normalization metadata for an observation.
#[derive(Deserialize)]
pub struct Provenance {
    #[serde(flatten)]
    pub acquisition: ActivityExportProvenance,
    pub transformations: Vec<ExportTransformation>,
    pub sequence_position: u32,
    pub creator: Creator,
}

/// Reads this application's versioned activity package
/// without extracting files or fetching URLs.
///
/// # Errors
/// [`enum@Error`] for malformed metadata/data,
/// unsupported profiles, unsafe paths, or integrity failures.
pub fn read(source: impl Read + Seek) -> Result<Package, Error> {
    let data = datapackage::Package::read_zip(source, datapackage::Limits::default())?;
    ensure(data.unlisted_members.is_empty(), "unlisted ZIP members")?;
    let report = data.validate();
    ensure(report.valid(), "data package validation failed")?;
    let descriptor = &data.descriptor;
    ensure(
        descriptor.schema.as_deref() == Some(datapackage::V2)
            && descriptor.name.as_deref() == Some("garmin-toolkit-activity")
            && descriptor.version.as_deref() == Some("0.1.0"),
        "unsupported activity package profile",
    )?;
    ensure(
        descriptor.metadata.get("provenance") == Some(&serde_json::json!("provenance.json")),
        "unsupported provenance path",
    )?;
    let selected: ObservationId = serde_json::from_value(
        descriptor
            .metadata
            .get("selectedObservationId")
            .cloned()
            .ok_or_else(|| invalid("missing selected observation"))?,
    )?;
    ensure(
        data.resources.iter().all(|resource| {
            resource.descriptor.hash.is_some() && resource.descriptor.bytes.is_some()
        }),
        "missing resource integrity metadata",
    )?;
    let mut resources = data
        .resources
        .into_iter()
        .map(|resource| (resource.descriptor.name.clone(), resource))
        .collect::<BTreeMap<_, _>>();
    let activities = table::<Aggregate>(&mut resources, "activities")?;
    let laps = table::<Aggregate>(&mut resources, "laps")?;
    let samples = table::<Sample>(&mut resources, "samples")?;
    let timer_events = table::<Event>(&mut resources, "timer-events")?;
    let provenance_resource = resources
        .remove("provenance")
        .ok_or_else(|| invalid("missing provenance"))?;
    ensure(
        provenance_resource.descriptor.path.as_deref() == Some("provenance.json")
            && provenance_resource.descriptor.format.as_deref() == Some("json"),
        "invalid provenance resource",
    )?;
    let provenance: Vec<Provenance> = serde_json::from_slice(&provenance_resource.bytes)?;
    let mut package = Package {
        selected,
        activities,
        laps,
        samples,
        timer_events,
        provenance,
        originals: BTreeMap::new(),
        tracks: BTreeMap::new(),
    };
    for resource in resources.into_values() {
        let metadata = &resource.descriptor;
        let path = metadata
            .path
            .as_deref()
            .ok_or_else(|| invalid("missing resource path"))?;
        if metadata.format.as_deref() == Some("fit") {
            let id = path_id::<ArtifactId>(path, "originals", "fit")?;
            ensure(
                metadata.name == format!("original-{id}")
                    && package.originals.insert(id, resource.bytes).is_none(),
                "invalid original resource",
            )?;
        } else if metadata.format.as_deref() == Some("gpx") {
            let id = path_id::<ObservationId>(path, "tracks", "gpx")?;
            ensure(
                metadata.name == format!("track-{id}")
                    && package.tracks.insert(id, resource.bytes).is_none(),
                "invalid track resource",
            )?;
        } else {
            return Err(invalid("unknown activity resource"));
        }
    }
    package.validate()?;
    Ok(package)
}

impl Package {
    fn validate_detail(&self, ids: &BTreeSet<ObservationId>) -> Result<(), Error> {
        for id in ids {
            ensure(
                self.laps.iter().any(|row| row.observation_id == *id)
                    && self.samples.iter().any(|row| row.observation_id == *id),
                "activity detail is missing",
            )?;
            let mut previous = None;
            for row in self.samples.iter().filter(|row| row.observation_id == *id) {
                ensure(
                    previous.is_none_or(|time| row.timestamp_ms >= time),
                    "samples are out of time order",
                )?;
                previous = Some(row.timestamp_ms);
                ensure(
                    match (row.latitude_degrees, row.longitude_degrees) {
                        (None, None) => true,
                        (Some(latitude), Some(longitude)) => {
                            (-90.0..=90.0).contains(&latitude)
                                && (-180.0..=180.0).contains(&longitude)
                        }
                        _ => false,
                    },
                    "invalid sample coordinate",
                )?;
                ensure(
                    row.elevation_m.is_none_or(f64::is_finite)
                        && row
                            .cadence_rpm
                            .is_none_or(|value| value.is_finite() && value >= 0.0),
                    "invalid sample measurement",
                )?;
            }
            let mut previous = None;
            for row in self
                .timer_events
                .iter()
                .filter(|row| row.observation_id == *id)
            {
                ensure(
                    previous.is_none_or(|time| row.timestamp_ms >= time)
                        && matches!(row.state.as_str(), "running" | "stopped"),
                    "invalid timer event",
                )?;
                previous = Some(row.timestamp_ms);
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        let ids = self
            .activities
            .iter()
            .map(|row| row.observation_id)
            .collect::<BTreeSet<_>>();
        ensure(
            ids.len() == self.activities.len() && ids.contains(&self.selected),
            "invalid activity primary key or selection",
        )?;
        ensure(
            self.activities.iter().all(|row| row.position == 0),
            "invalid activity position",
        )?;
        for row in self.activities.iter().chain(&self.laps) {
            ensure(
                row.end_ms >= row.start_ms && row.timer_ms <= row.elapsed_ms,
                "invalid aggregate timing",
            )?;
            ensure(
                matches!(
                    row.sport.as_str(),
                    "running" | "cycling" | "swimming" | "walking" | "hiking"
                ),
                "invalid activity sport",
            )?;
            ensure(
                [row.average_cadence_rpm, row.maximum_cadence_rpm]
                    .into_iter()
                    .flatten()
                    .all(|value| value.is_finite() && value >= 0.0),
                "invalid aggregate cadence",
            )?;
        }
        validate_positions(
            self.laps
                .iter()
                .map(|row| (row.observation_id, row.position)),
            &ids,
        )?;
        validate_positions(
            self.samples
                .iter()
                .map(|row| (row.observation_id, row.position)),
            &ids,
        )?;
        validate_positions(
            self.timer_events
                .iter()
                .map(|row| (row.observation_id, row.position)),
            &ids,
        )?;
        self.validate_detail(&ids)?;
        let provenance_ids = self
            .provenance
            .iter()
            .map(|row| row.acquisition.observation_id)
            .collect::<BTreeSet<_>>();
        ensure(
            provenance_ids == ids && self.provenance.len() == ids.len(),
            "provenance does not cover activities",
        )?;
        ensure(
            self.tracks.keys().copied().collect::<BTreeSet<_>>() == ids,
            "track resources do not cover activities",
        )?;
        let original_ids = self
            .provenance
            .iter()
            .map(|row| row.acquisition.artifact_id)
            .collect::<BTreeSet<_>>();
        ensure(
            self.originals.keys().copied().collect::<BTreeSet<_>>() == original_ids,
            "originals do not cover provenance",
        )?;
        for row in &self.provenance {
            let original = &self.originals[&row.acquisition.artifact_id];
            ensure(
                original.len() as u64 == row.acquisition.byte_count
                    && ArtifactDigest::from_bytes(original).to_string()
                        == row.acquisition.artifact_blake3,
                "original FIT provenance mismatch",
            )?;
        }
        Ok(())
    }
}

fn validate_positions(
    rows: impl Iterator<Item = (ObservationId, usize)>,
    ids: &BTreeSet<ObservationId>,
) -> Result<(), Error> {
    let mut positions = BTreeMap::<ObservationId, usize>::new();
    for (id, position) in rows {
        let expected = positions.entry(id).or_default();
        ensure(
            ids.contains(&id) && position == *expected,
            "invalid foreign key or row order",
        )?;
        *expected += 1;
    }
    Ok(())
}

fn table<T: Table + DeserializeOwned>(
    resources: &mut BTreeMap<String, datapackage::Resource>,
    name: &str,
) -> Result<Vec<T>, Error> {
    let resource = resources
        .remove(name)
        .ok_or_else(|| invalid("missing CSV relation"))?;
    ensure(
        resource.descriptor.path.as_deref() == Some(format!("data/{name}.csv").as_str())
            && resource.descriptor.format.as_deref() == Some("csv"),
        "invalid relation resource",
    )?;
    let schema = serde_json::to_value(
        resource
            .descriptor
            .schema
            .as_ref()
            .ok_or_else(|| invalid("missing relation schema"))?,
    )?;
    let expected_primary = if name == "activities" {
        serde_json::json!(["observation_id"])
    } else {
        serde_json::json!(["observation_id", "position"])
    };
    let expected_foreign = (name != "activities").then(|| {
        serde_json::json!([{
            "fields": ["observation_id"],
            "reference": {"resource": "activities", "fields": ["observation_id"]},
        }])
    });
    ensure(
        schema["fields"] == serde_json::to_value(T::fields())?
            && schema["missingValues"] == serde_json::json!([""])
            && schema["primaryKey"] == expected_primary
            && schema.get("foreignKeys") == expected_foreign.as_ref(),
        "unsupported relation schema",
    )?;
    Ok(resource.deserialize()?)
}

fn path_id<T: std::str::FromStr>(path: &str, directory: &str, extension: &str) -> Result<T, Error> {
    let path = Path::new(path);
    ensure(
        path.parent() == Some(Path::new(directory))
            && path.extension().and_then(|value| value.to_str()) == Some(extension),
        "invalid resource location",
    )?;
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse().ok())
        .ok_or_else(|| invalid("invalid resource identifier"))
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.to_owned())
}
fn ensure(valid: bool, message: &str) -> Result<(), Error> {
    if valid { Ok(()) } else { Err(invalid(message)) }
}
