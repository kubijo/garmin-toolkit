//! Owner-scoped activity material and its immutable domain provenance.

use garmin_model::{
    artifact::{
        Acquisition, Artifact, NormalizationOutcome, NormalizationRun, NormalizationRunId,
        SchemaVersion,
    },
    identity::{Source, UserId},
    observation::{FingerprintSchema, ObservationId, SemanticFingerprint},
    value::{ComponentVersion, Transformation},
};

use crate::{Storage, StoredActivity};

/// Complete activity material authorized through one profile, without a file-format representation.
pub struct ActivityExport {
    pub activity: StoredActivity,
    pub source: Source,
    pub acquisition: Acquisition,
    pub normalization: NormalizationRun,
    pub artifact: Artifact,
    pub fingerprint: SemanticFingerprint,
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
            let artifact = Artifact::from_bytes(
                super::read::parse(&row.artifact_id, "export artifact ID")?,
                super::read::parse(&row.media_type, "export media type")?,
                &row.bytes,
            );
            if artifact.byte_count().as_u64()
                != u64::try_from(row.byte_count)
                    .map_err(|error| super::read::invalid("export artifact byte count", error))?
                || artifact.digest().as_blake3().as_bytes().as_slice() != row.digest
            {
                return Err(super::read::invalid(
                    "export artifact",
                    "original bytes failed integrity verification",
                ));
            }
            let source = Source::from_parts(
                super::read::parse(&row.source_id, "export source ID")?,
                owner,
                super::read::parse(&row.source_label, "export source label")?,
                row.device_id
                    .map(|id| super::read::parse(&id, "export device ID"))
                    .transpose()?,
            );
            let acquisition = Acquisition::from_source(
                super::read::parse(&row.acquisition_id, "export acquisition ID")?,
                super::read::parse(&row.operation_id, "export operation ID")?,
                artifact.id(),
                &source,
                super::read::parse(&row.source_identity, "export source identity")?,
                super::read::parse(&row.acquired_at, "export acquisition timestamp")?,
            );
            let normalization_run_id: NormalizationRunId =
                super::read::parse(&row.normalization_run_id, "export normalization ID")?;
            let transformations = self
                .export_transformations(&row.normalization_run_id)
                .await?;
            // The observation foreign key requires a successful normalization run.
            let normalization = NormalizationRun::from_parts(
                normalization_run_id,
                &acquisition,
                component(row.parser_name, &row.parser_version, "export parser")?,
                SchemaVersion::from_u32(
                    row.schema_version
                        .try_into()
                        .map_err(|error| super::read::invalid("export schema", error))?,
                )
                .map_err(|error| super::read::invalid("export schema", error))?,
                transformations,
                NormalizationOutcome::Succeeded,
            );
            let fingerprint = SemanticFingerprint::from_parts(
                FingerprintSchema::from_component(component(
                    row.fingerprint_schema_name,
                    &row.fingerprint_schema_version,
                    "export fingerprint schema",
                )?),
                blake3::Hash::from_bytes(row.fingerprint_digest.try_into().map_err(|_| {
                    super::read::invalid("export fingerprint", "invalid digest length")
                })?),
            );
            exports.push(ActivityExport {
                activity,
                source,
                acquisition,
                normalization,
                artifact,
                fingerprint,
                original: row.bytes,
            });
        }
        Ok(exports)
    }

    async fn export_transformations(
        &self,
        normalization_run_id: &str,
    ) -> Result<Vec<Transformation>, crate::Error> {
        sqlx::query_file!("queries/export-transformations.sql", normalization_run_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .enumerate()
            .map(|(position, row)| {
                if usize::try_from(row.position).ok() != Some(position) {
                    return Err(super::read::invalid(
                        "export transformation",
                        "non-contiguous positions",
                    ));
                }
                component(
                    row.component_name,
                    &row.component_version,
                    "export transformation",
                )
                .map(Transformation::from_component)
            })
            .collect()
    }
}

fn component(
    name: String,
    version: &str,
    field: &'static str,
) -> Result<ComponentVersion, crate::Error> {
    ComponentVersion::from_parts(
        name,
        version
            .parse()
            .map_err(|error| super::read::invalid(field, error))?,
    )
    .map_err(|error| super::read::invalid(field, error))
}
