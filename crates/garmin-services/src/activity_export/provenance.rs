//! Serialization owned by the versioned activity package profile.

use garmin_model::{
    artifact::{
        AcquisitionId, AcquisitionOperationId, ArtifactId, NormalizationRunId, SourceIdentity,
    },
    identity::{DeviceId, SourceId},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_storage::ActivityExport;
use serde::{Deserialize, Serialize};

use super::{Creator, Error, ParsedProvenance};

/// Acquisition and normalization fields in the activity package profile.
#[derive(Deserialize, Serialize)]
pub struct ActivityExportProvenance {
    pub observation_id: ObservationId,
    pub normalization_run_id: NormalizationRunId,
    pub acquisition_id: AcquisitionId,
    pub operation_id: AcquisitionOperationId,
    pub artifact_id: ArtifactId,
    pub source_id: SourceId,
    #[serde(
        serialize_with = "serialize_source_identity",
        deserialize_with = "deserialize_source_identity"
    )]
    pub source_identity: SourceIdentity,
    pub source_label: String,
    pub device_id: Option<DeviceId>,
    pub acquired_at: Timestamp,
    pub parser_name: String,
    pub parser_version: String,
    pub schema_version: u32,
    pub fingerprint_schema_name: String,
    pub fingerprint_schema_version: String,
    pub fingerprint_blake3: String,
    pub media_type: String,
    pub artifact_blake3: String,
    pub byte_count: u64,
}

/// One ordered normalization transformation in the activity package profile.
#[derive(Deserialize, Serialize)]
pub struct ExportTransformation {
    pub normalization_run_id: NormalizationRunId,
    pub position: u32,
    pub component_name: String,
    pub component_version: String,
}

impl From<&ActivityExport> for ActivityExportProvenance {
    fn from(recording: &ActivityExport) -> Self {
        let parser = recording.normalization.parser();
        let fingerprint_schema = recording.fingerprint.schema().as_component();
        Self {
            observation_id: recording.activity.observation_id(),
            normalization_run_id: recording.normalization.id(),
            acquisition_id: recording.acquisition.id(),
            operation_id: recording.acquisition.operation_id(),
            artifact_id: recording.artifact.id(),
            source_id: recording.source.id(),
            source_identity: recording.acquisition.source_identity().clone(),
            source_label: recording.source.label().to_string(),
            device_id: recording.source.device_id(),
            acquired_at: recording.acquisition.acquired_at(),
            parser_name: parser.name().to_owned(),
            parser_version: parser.version().to_string(),
            schema_version: recording.normalization.schema().as_u32(),
            fingerprint_schema_name: fingerprint_schema.name().to_owned(),
            fingerprint_schema_version: fingerprint_schema.version().to_string(),
            fingerprint_blake3: recording.fingerprint.digest().to_hex().to_string(),
            media_type: recording.artifact.media_type().to_string(),
            artifact_blake3: recording.artifact.digest().to_string(),
            byte_count: recording.artifact.byte_count().as_u64(),
        }
    }
}

impl TryFrom<&ActivityExport> for ParsedProvenance {
    type Error = Error;

    fn try_from(recording: &ActivityExport) -> Result<Self, Self::Error> {
        let normalization_run_id = recording.normalization.id();
        let transformations = recording
            .normalization
            .transformations()
            .iter()
            .enumerate()
            .map(|(position, transformation)| {
                let component = transformation.as_component();
                Ok(ExportTransformation {
                    normalization_run_id,
                    position: position.try_into().map_err(|_| {
                        Error::Invalid("too many normalization transformations".to_owned())
                    })?,
                    component_name: component.name().to_owned(),
                    component_version: component.version().to_string(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            acquisition: ActivityExportProvenance::from(recording),
            transformations,
            sequence_position: recording.activity.sequence_position().as_u32(),
            creator: Creator::from(recording.activity.normalized().creator()),
        })
    }
}

fn serialize_source_identity<S: serde::Serializer>(
    identity: &SourceIdentity,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(identity.as_str())
}

fn deserialize_source_identity<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SourceIdentity, D::Error> {
    let value = String::deserialize(deserializer)?;
    SourceIdentity::from_string(value).map_err(serde::de::Error::custom)
}
