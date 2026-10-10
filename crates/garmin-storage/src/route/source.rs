//! Original imported bytes remain independently downloadable after route edits.

use garmin_model::{
    artifact::{ArtifactDigest, ArtifactId, ByteCount},
    identity::UserId,
    route::RoutePlanId,
};

use crate::{Storage, route::RoutePersistenceError};

/// Metadata for the original source of a route plan.
pub struct RouteSource {
    pub artifact: ArtifactId,
    pub digest: ArtifactDigest,
    pub byte_count: ByteCount,
    pub name: String,
}

impl Storage {
    /// Reads original source metadata through its owning route.
    /// # Errors
    /// Fails on database errors or malformed persisted metadata.
    pub async fn route_source(
        &self,
        owner: UserId,
        plan: RoutePlanId,
    ) -> Result<Option<RouteSource>, crate::Error> {
        let owner = owner.to_string();
        let plan = plan.to_string();
        let Some(row) = sqlx::query_file!("queries/route-source.sql", owner, plan)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(RouteSource {
            artifact: row.id.parse().map_err(invalid)?,
            digest: digest(row.digest)?,
            byte_count: ByteCount::from_u64(u64::try_from(row.byte_count).map_err(invalid)?),
            name: row.source_identity,
        }))
    }

    /// Returns verified original bytes only when referenced by an owned route.
    /// # Errors
    /// Fails on database errors or corrupted stored bytes.
    pub async fn route_source_artifact(
        &self,
        owner: UserId,
        artifact: ArtifactId,
    ) -> Result<Option<Vec<u8>>, crate::Error> {
        let owner = owner.to_string();
        let artifact = artifact.to_string();
        let Some(row) = sqlx::query_file!("queries/route-source-artifact.sql", artifact, owner)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        if u64::try_from(row.byte_count).ok() != Some(row.bytes.len() as u64)
            || ArtifactDigest::from_bytes(&row.bytes) != digest(row.digest)?
        {
            return Err(RoutePersistenceError::ArtifactBytesMismatch.into());
        }
        Ok(Some(row.bytes))
    }
}

fn digest(bytes: Vec<u8>) -> Result<ArtifactDigest, crate::Error> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid("invalid artifact digest"))?;
    Ok(ArtifactDigest::from_blake3(blake3::Hash::from_bytes(bytes)))
}

fn invalid(error: impl std::fmt::Display) -> crate::Error {
    super::invalid("route source", error)
}
