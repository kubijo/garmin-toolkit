//! Owner-scoped export material and its immutable source provenance.

use garmin_model::{
    artifact::{
        AcquisitionId, AcquisitionOperationId, ArtifactDigest, ArtifactId, NormalizationRunId,
        SchemaVersion, SourceIdentity,
    },
    identity::{DeviceId, SourceId, UserId},
    observation::ObservationId,
    value::{ComponentVersion, Timestamp},
};
use serde::{Deserialize, Serialize};

use crate::{Storage, StoredActivity};

/// Provenance for one exported activity observation.
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

/// One normalization transformation in source order.
#[derive(Deserialize, Serialize)]
pub struct ExportTransformation {
    pub normalization_run_id: NormalizationRunId,
    pub position: u32,
    pub component_name: String,
    pub component_version: String,
}

/// Complete immutable activity material authorized through one profile.
pub struct ActivityExport {
    pub activity: StoredActivity,
    pub provenance: ActivityExportProvenance,
    pub transformations: Vec<ExportTransformation>,
    pub original: Vec<u8>,
}

impl Storage {
    /// Loads the selected activity and associated recordings within its owner's scope.
    /// # Errors
    /// [`enum@crate::Error`] for invalid stored data, missing projections, or source-byte corruption.
    pub async fn activity_export(
        &self,
        owner: UserId,
        selected: ObservationId,
    ) -> Result<Vec<ActivityExport>, crate::Error> {
        let owner_text = owner.to_string();
        let selected = selected.to_string();
        let rows = sqlx::query_file!("queries/activity-export.sql", owner_text, selected)
            .fetch_all(&self.pool)
            .await?;
        let mut exports = Vec::with_capacity(rows.len());
        for row in rows {
            let observation_id: ObservationId =
                super::read::parse(&row.observation_id, "export observation ID")?;
            let activity = self.activity(owner, observation_id).await?.ok_or_else(|| {
                super::read::invalid("export activity", "associated projection is missing")
            })?;
            let digest: [u8; 32] = row
                .digest
                .try_into()
                .map_err(|_| super::read::invalid("export artifact", "invalid digest length"))?;
            let byte_count: u64 = row
                .byte_count
                .try_into()
                .map_err(|_| super::read::invalid("export artifact", "invalid byte count"))?;
            if row.bytes.len() as u64 != byte_count
                || ArtifactDigest::from_bytes(&row.bytes)
                    .as_blake3()
                    .as_bytes()
                    != &digest
            {
                return Err(super::read::invalid(
                    "export artifact",
                    "original bytes failed integrity verification",
                ));
            }
            let normalization_run_id: NormalizationRunId =
                super::read::parse(&row.normalization_run_id, "export normalization ID")?;
            let transformations = sqlx::query_file!(
                "queries/export-transformations.sql",
                row.normalization_run_id
            )
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(ExportTransformation {
                    normalization_run_id,
                    position: row.position.try_into().map_err(|_| {
                        super::read::invalid("export transformation", "invalid position")
                    })?,
                    component_name: row.component_name,
                    component_version: row.component_version,
                })
            })
            .collect::<Result<Vec<_>, crate::Error>>()?;
            // Decode persisted typed values before exposing them to a portable consumer.
            let parser = ComponentVersion::from_parts(
                row.parser_name.clone(),
                row.parser_version
                    .parse()
                    .map_err(|error| super::read::invalid("export parser version", error))?,
            )
            .map_err(|error| super::read::invalid("export parser", error))?;
            let schema =
                SchemaVersion::from_u32(row.schema_version.try_into().map_err(|_| {
                    super::read::invalid("export schema", "invalid schema version")
                })?)
                .map_err(|error| super::read::invalid("export schema", error))?;
            let provenance = ActivityExportProvenance {
                observation_id,
                normalization_run_id,
                acquisition_id: super::read::parse(&row.acquisition_id, "export acquisition ID")?,
                operation_id: super::read::parse(&row.operation_id, "export operation ID")?,
                artifact_id: super::read::parse(&row.artifact_id, "export artifact ID")?,
                source_id: super::read::parse(&row.source_id, "export source ID")?,
                source_identity: super::read::parse(
                    &row.source_identity,
                    "export source identity",
                )?,
                source_label: row.source_label,
                device_id: row
                    .device_id
                    .map(|id| super::read::parse(&id, "export device ID"))
                    .transpose()?,
                acquired_at: super::read::parse(&row.acquired_at, "export acquisition timestamp")?,
                parser_name: parser.name().to_owned(),
                parser_version: parser.version().to_string(),
                schema_version: schema.as_u32(),
                fingerprint_schema_name: row.fingerprint_schema_name,
                fingerprint_schema_version: row.fingerprint_schema_version,
                fingerprint_blake3: hex::encode(row.fingerprint_digest),
                media_type: row.media_type,
                artifact_blake3: hex::encode(digest),
                byte_count,
            };
            exports.push(ActivityExport {
                activity,
                provenance,
                transformations,
                original: row.bytes,
            });
        }
        Ok(exports)
    }
}
