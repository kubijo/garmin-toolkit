//! Profile-avatar persistence.

use garmin_model::{
    artifact::{Acquisition, Artifact, ArtifactDigest, ArtifactId, ByteCount},
    identity::{AvatarArtifactId, ImageDimensions, UserId},
};
use thiserror::Error;

use crate::{
    Storage,
    ingestion::{persist_acquisition, persist_artifact, persist_blob},
};

/// One original avatar and its generated display thumbnail.
pub struct ProfileAvatar<'a> {
    original: &'a Artifact,
    original_bytes: &'a [u8],
    acquisition: &'a Acquisition,
    original_dimensions: ImageDimensions,
    thumbnail: &'a Artifact,
    thumbnail_bytes: &'a [u8],
}

impl<'a> ProfileAvatar<'a> {
    /// Validates a complete avatar persistence boundary.
    /// # Errors
    /// [`AvatarPersistenceError`] when records and bytes disagree.
    pub fn from_parts(
        original: &'a Artifact,
        original_bytes: &'a [u8],
        acquisition: &'a Acquisition,
        original_dimensions: ImageDimensions,
        thumbnail: &'a Artifact,
        thumbnail_bytes: &'a [u8],
    ) -> Result<Self, AvatarPersistenceError> {
        validate_bytes(original, original_bytes)?;
        validate_bytes(thumbnail, thumbnail_bytes)?;
        if acquisition.artifact_id() != original.id() {
            return Err(AvatarPersistenceError::AcquisitionArtifactMismatch);
        }
        if original.id() == thumbnail.id() {
            return Err(AvatarPersistenceError::ArtifactIdsMatch);
        }
        if !matches!(
            original.media_type().to_string().as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        ) {
            return Err(AvatarPersistenceError::OriginalMediaType);
        }
        if thumbnail.media_type().to_string() != "image/png" {
            return Err(AvatarPersistenceError::ThumbnailMediaType);
        }
        Ok(Self {
            original,
            original_bytes,
            acquisition,
            original_dimensions,
            thumbnail,
            thumbnail_bytes,
        })
    }

    const fn owner_id(&self) -> UserId {
        self.acquisition.owner_id()
    }
}

/// Persisted selected avatar data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredProfileAvatar {
    original_artifact_id: AvatarArtifactId,
    thumbnail_artifact_id: ArtifactId,
    original_dimensions: ImageDimensions,
    thumbnail_bytes: Vec<u8>,
}

impl StoredProfileAvatar {
    #[must_use]
    pub const fn original_artifact_id(&self) -> AvatarArtifactId {
        self.original_artifact_id
    }

    #[must_use]
    pub const fn thumbnail_artifact_id(&self) -> ArtifactId {
        self.thumbnail_artifact_id
    }

    #[must_use]
    pub const fn original_dimensions(&self) -> ImageDimensions {
        self.original_dimensions
    }

    #[must_use]
    pub fn thumbnail_bytes(&self) -> &[u8] {
        &self.thumbnail_bytes
    }

    #[must_use]
    pub fn into_thumbnail_bytes(self) -> Vec<u8> {
        self.thumbnail_bytes
    }
}

impl Storage {
    /// Stores and selects an avatar atomically.
    ///
    /// Exact retries are idempotent.
    /// Originals and thumbnails are immutable.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or immutable conflicts.
    pub async fn save_profile_avatar(&self, avatar: ProfileAvatar<'_>) -> Result<(), crate::Error> {
        let mut transaction = self.pool.begin().await?;
        persist_blob(&mut transaction, avatar.original, avatar.original_bytes).await?;
        persist_artifact(&mut transaction, avatar.original).await?;
        persist_acquisition(&mut transaction, avatar.acquisition).await?;
        persist_blob(&mut transaction, avatar.thumbnail, avatar.thumbnail_bytes).await?;
        persist_artifact(&mut transaction, avatar.thumbnail).await?;

        let owner_id = avatar.owner_id().to_string();
        let acquisition_id = avatar.acquisition.id().to_string();
        let original_id = avatar.original.id().to_string();
        let thumbnail_id = avatar.thumbnail.id().to_string();
        let original_width = i64::from(avatar.original_dimensions.width());
        let original_height = i64::from(avatar.original_dimensions.height());
        let stored = sqlx::query_file!(
            "queries/persist-profile-avatar.sql",
            owner_id,
            acquisition_id,
            original_id,
            thumbnail_id,
            original_width,
            original_height,
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if stored.is_none() {
            return Err(AvatarPersistenceError::AvatarConflict.into());
        }

        let selected = sqlx::query_file!(
            "queries/select-profile-avatar.sql",
            original_id,
            owner_id,
            original_id,
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if selected.is_none() {
            return Err(AvatarPersistenceError::UserNotFound.into());
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Clears the selected avatar without deleting retained artifacts.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or an unknown user.
    pub async fn clear_profile_avatar(&self, owner_id: UserId) -> Result<(), crate::Error> {
        let owner_id = owner_id.to_string();
        let cleared = sqlx::query_file!("queries/clear-profile-avatar.sql", owner_id)
            .fetch_optional(&self.pool)
            .await?;
        if cleared.is_none() {
            return Err(AvatarPersistenceError::UserNotFound.into());
        }
        Ok(())
    }

    /// Loads the selected avatar and its display thumbnail.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted data.
    pub async fn profile_avatar(
        &self,
        owner_id: UserId,
    ) -> Result<Option<StoredProfileAvatar>, crate::Error> {
        let owner_id = owner_id.to_string();
        let Some(row) = sqlx::query_file!("queries/profile-avatar.sql", owner_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let original_artifact_id = row
            .original_artifact_id
            .parse::<ArtifactId>()
            .map(AvatarArtifactId::from_artifact_id)
            .map_err(|error| crate::Error::InvalidData {
                field: "profile avatar artifact ID",
                reason: error.to_string(),
            })?;
        let thumbnail_artifact_id =
            row.thumbnail_artifact_id
                .parse::<ArtifactId>()
                .map_err(|error| crate::Error::InvalidData {
                    field: "profile avatar thumbnail artifact ID",
                    reason: error.to_string(),
                })?;
        let width =
            u32::try_from(row.original_width).map_err(|error| crate::Error::InvalidData {
                field: "profile avatar width",
                reason: error.to_string(),
            })?;
        let height =
            u32::try_from(row.original_height).map_err(|error| crate::Error::InvalidData {
                field: "profile avatar height",
                reason: error.to_string(),
            })?;
        let original_dimensions =
            ImageDimensions::from_width_height(width, height).map_err(|error| {
                crate::Error::InvalidData {
                    field: "profile avatar dimensions",
                    reason: error.to_string(),
                }
            })?;
        Ok(Some(StoredProfileAvatar {
            original_artifact_id,
            thumbnail_artifact_id,
            original_dimensions,
            thumbnail_bytes: row.thumbnail_bytes,
        }))
    }
}

fn validate_bytes(artifact: &Artifact, bytes: &[u8]) -> Result<(), AvatarPersistenceError> {
    if artifact.digest() != ArtifactDigest::from_bytes(bytes)
        || artifact.byte_count() != ByteCount::from_u64(bytes.len() as u64)
    {
        return Err(AvatarPersistenceError::ArtifactBytesMismatch);
    }
    Ok(())
}

/// Invalid avatar persistence data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AvatarPersistenceError {
    #[error("avatar artifact metadata does not match its bytes")]
    ArtifactBytesMismatch,
    #[error("avatar acquisition references another artifact")]
    AcquisitionArtifactMismatch,
    #[error("avatar original and thumbnail artifact IDs must differ")]
    ArtifactIdsMatch,
    #[error("avatar original media type must be image/png, image/jpeg, or image/webp")]
    OriginalMediaType,
    #[error("avatar thumbnail media type must be image/png")]
    ThumbnailMediaType,
    #[error("avatar record conflicts with an existing immutable record")]
    AvatarConflict,
    #[error("avatar owner does not exist")]
    UserNotFound,
}
