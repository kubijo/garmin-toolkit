//! Durable selected-candidate identity, provenance, and retry receipts.

use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest},
    identity::{Source, UserId},
    route::{RouteCandidateSource, RouteImportIdentity, RouteImportReceipt},
    value::{ComponentVersion, Timestamp},
};
use sqlx::{Sqlite, SqliteConnection, Transaction};

use super::{RouteImport, RoutePersistenceError, decode_sport, invalid, persist_import};
use crate::Storage;

#[cfg(test)]
mod tests;

/// A committed selection and the parser that produced its initial revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRouteImport {
    identity: RouteImportIdentity,
    receipt: RouteImportReceipt,
    parser: ComponentVersion,
    acquired_at: Timestamp,
}

impl StoredRouteImport {
    #[must_use]
    pub const fn identity(&self) -> &RouteImportIdentity {
        &self.identity
    }
    #[must_use]
    pub const fn receipt(&self) -> RouteImportReceipt {
        self.receipt
    }
    #[must_use]
    pub const fn parser(&self) -> &ComponentVersion {
        &self.parser
    }
    #[must_use]
    pub const fn acquired_at(&self) -> Timestamp {
        self.acquired_at
    }

    /// Returns the original receipt only for the identical confirmed request.
    /// # Errors
    /// [`RoutePersistenceError::ImportOperationConflict`] for changed arguments.
    pub fn retry(
        &self,
        identity: &RouteImportIdentity,
    ) -> Result<RouteImportReceipt, RoutePersistenceError> {
        if &self.identity != identity {
            return Err(RoutePersistenceError::ImportOperationConflict);
        }
        Ok(self.receipt)
    }
}

impl Storage {
    /// Looks up a committed import before a preview or parser is needed again.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted data.
    pub async fn route_import(
        &self,
        owner: UserId,
        operation: AcquisitionOperationId,
    ) -> Result<Option<StoredRouteImport>, crate::Error> {
        read_import(&mut *self.pool.acquire().await?, owner, operation).await
    }

    /// Commits source registration, original bytes, route, and selection receipt together.
    /// Concurrent identical requests return the first committed result without moving the head.
    /// # Errors
    /// [`enum@crate::Error`] for invalid identity, immutable conflicts, or database failure.
    pub async fn save_selected_route_import(
        &self,
        source: &Source,
        identity: &RouteImportIdentity,
        parser: &ComponentVersion,
        import: RouteImport<'_>,
    ) -> Result<RouteImportReceipt, crate::Error> {
        validate_identity(source, identity, &import)?;
        let mut transaction = self
            .pool
            .begin_with(include_str!("../../queries/begin-immediate.sql"))
            .await?;
        if let Some(existing) = read_import(
            &mut transaction,
            source.owner_id(),
            import.acquisition.operation_id(),
        )
        .await?
        {
            let receipt = existing.retry(identity)?;
            transaction.commit().await?;
            return Ok(receipt);
        }
        crate::persist_source(&mut transaction, source).await?;
        persist_import(&mut transaction, &import).await?;
        insert_receipt(&mut transaction, identity.candidate(), parser, &import).await?;
        transaction.commit().await?;
        Ok(RouteImportReceipt::from_parts(
            import.artifact.id(),
            import.acquisition.id(),
            import.plan.id(),
            import.revision.id(),
        ))
    }
}

fn validate_identity(
    source: &Source,
    identity: &RouteImportIdentity,
    import: &RouteImport<'_>,
) -> Result<(), RoutePersistenceError> {
    let expected = RouteImportIdentity::from_parts(
        import.acquisition.owner_id(),
        import.acquisition.source_id(),
        import.acquisition.source_identity().clone(),
        import.artifact.digest(),
        identity.candidate(),
        import.revision.name().clone(),
        import.revision.sport(),
    );
    if expected != *identity
        || source.id() != identity.source_id()
        || source.owner_id() != identity.owner_id()
    {
        return Err(RoutePersistenceError::ImportIdentityMismatch);
    }
    Ok(())
}

async fn read_import(
    connection: &mut SqliteConnection,
    owner: UserId,
    operation: AcquisitionOperationId,
) -> Result<Option<StoredRouteImport>, crate::Error> {
    let owner = owner.to_string();
    let operation = operation.to_string();
    sqlx::query_file_as!(
        ImportRow,
        "queries/route-import-receipt.sql",
        owner,
        operation
    )
    .fetch_optional(connection)
    .await?
    .map(ImportRow::decode)
    .transpose()
}

async fn insert_receipt(
    transaction: &mut Transaction<'_, Sqlite>,
    candidate: RouteCandidateSource,
    parser: &ComponentVersion,
    import: &RouteImport<'_>,
) -> Result<(), crate::Error> {
    let (kind, index, segment) = match candidate {
        RouteCandidateSource::TrackSegment { track, segment } => {
            ("track_segment", track, Some(segment))
        }
        RouteCandidateSource::Route { route } => ("route", route, None),
    };
    let index = i64::try_from(index).map_err(|_| RoutePersistenceError::TooManyRecords)?;
    let segment = segment
        .map(i64::try_from)
        .transpose()
        .map_err(|_| RoutePersistenceError::TooManyRecords)?;
    let acquisition = import.acquisition.id().to_string();
    let artifact = import.artifact.id().to_string();
    let owner = import.plan.owner_id().to_string();
    let plan = import.plan.id().to_string();
    let revision = import.revision.id().to_string();
    let parser_name = parser.name();
    let parser_version = parser.version().to_string();
    sqlx::query_file!(
        "queries/insert-route-import-receipt.sql",
        acquisition,
        artifact,
        owner,
        plan,
        revision,
        kind,
        index,
        segment,
        parser_name,
        parser_version
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

struct ImportRow {
    acquisition_id: String,
    artifact_id: String,
    owner_id: String,
    source_id: String,
    source_identity: String,
    acquired_at: String,
    digest: Vec<u8>,
    plan_id: String,
    revision_id: String,
    candidate_kind: String,
    candidate_index: i64,
    segment_index: Option<i64>,
    parser_name: String,
    parser_version: String,
    name: String,
    sport: String,
}

impl ImportRow {
    fn decode(self) -> Result<StoredRouteImport, crate::Error> {
        let index = usize::try_from(self.candidate_index).map_err(data_error)?;
        let candidate = match (self.candidate_kind.as_str(), self.segment_index) {
            ("track_segment", Some(segment)) => RouteCandidateSource::TrackSegment {
                track: index,
                segment: usize::try_from(segment).map_err(data_error)?,
            },
            ("route", None) => RouteCandidateSource::Route { route: index },
            _ => return Err(data_error("invalid candidate locator")),
        };
        let digest: [u8; 32] = self
            .digest
            .try_into()
            .map_err(|_| data_error("invalid digest length"))?;
        Ok(StoredRouteImport {
            identity: RouteImportIdentity::from_parts(
                self.owner_id.parse().map_err(data_error)?,
                self.source_id.parse().map_err(data_error)?,
                self.source_identity.parse().map_err(data_error)?,
                ArtifactDigest::from_blake3(blake3::Hash::from_bytes(digest)),
                candidate,
                self.name.parse().map_err(data_error)?,
                decode_sport(&self.sport)?,
            ),
            receipt: RouteImportReceipt::from_parts(
                self.artifact_id.parse().map_err(data_error)?,
                self.acquisition_id.parse().map_err(data_error)?,
                self.plan_id.parse().map_err(data_error)?,
                self.revision_id.parse().map_err(data_error)?,
            ),
            parser: ComponentVersion::from_parts(
                self.parser_name,
                self.parser_version.parse().map_err(data_error)?,
            )
            .map_err(data_error)?,
            acquired_at: self.acquired_at.parse().map_err(data_error)?,
        })
    }
}

fn data_error(error: impl std::fmt::Display) -> crate::Error {
    invalid("route import receipt", error)
}
