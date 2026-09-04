//! Profile-avatar image import.

use std::io::Cursor;

use garmin_model::{
    artifact::{
        Acquisition, AcquisitionId, AcquisitionIdKind, AcquisitionOperationId, Artifact,
        ArtifactId, ArtifactIdKind, MediaType, SourceIdentity,
    },
    identity::{AvatarArtifactId, ImageDimensions, ImageDimensionsError, Source, UserId},
    value::Timestamp,
};
use garmin_storage::{ProfileAvatar, Storage};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, imageops::FilterType};
use newtype_uuid::{GenericUuid, TypedUuid, TypedUuidKind};
use thiserror::Error;
use uuid::Uuid;

/// Maximum accepted encoded avatar size.
pub const MAX_AVATAR_BYTES: usize = 10 * 1024 * 1024;
/// Maximum accepted oriented width or height.
pub const MAX_AVATAR_DIMENSION: u32 = 4_096;
/// Generated square thumbnail edge length.
pub const THUMBNAIL_EDGE: u32 = 256;
const MAX_DECODE_ALLOCATION: u64 = 128 * 1024 * 1024;
const PNG_MEDIA_TYPE: &str = "image/png";
const THUMBNAIL_PIPELINE_VERSION: u32 = 1;

/// A square crop in oriented source-image pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AvatarCrop {
    left: u32,
    top: u32,
    edge: u32,
}

impl AvatarCrop {
    /// Creates a non-empty square crop.
    /// # Errors
    /// [`AvatarCropError`] when `edge` is zero.
    pub const fn from_pixels(left: u32, top: u32, edge: u32) -> Result<Self, AvatarCropError> {
        if edge == 0 {
            return Err(AvatarCropError::Empty);
        }
        Ok(Self { left, top, edge })
    }

    #[must_use]
    pub const fn left(self) -> u32 {
        self.left
    }

    #[must_use]
    pub const fn top(self) -> u32 {
        self.top
    }

    #[must_use]
    pub const fn edge(self) -> u32 {
        self.edge
    }
}

/// Invalid avatar crop geometry.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AvatarCropError {
    #[error("avatar crop edge must be non-zero")]
    Empty,
    #[error("avatar crop exceeds the oriented source image")]
    OutsideImage,
}

/// One profile-avatar acquisition request.
pub struct AvatarImportRequest<'a> {
    actor_id: UserId,
    source: &'a Source,
    source_identity: SourceIdentity,
    operation_id: AcquisitionOperationId,
    acquired_at: Timestamp,
    bytes: &'a [u8],
    crop: Option<AvatarCrop>,
}

impl<'a> AvatarImportRequest<'a> {
    #[must_use]
    pub const fn from_parts(
        actor_id: UserId,
        source: &'a Source,
        source_identity: SourceIdentity,
        operation_id: AcquisitionOperationId,
        acquired_at: Timestamp,
        bytes: &'a [u8],
    ) -> Self {
        Self {
            actor_id,
            source,
            source_identity,
            operation_id,
            acquired_at,
            bytes,
            crop: None,
        }
    }

    #[must_use]
    pub const fn crop(mut self, crop: AvatarCrop) -> Self {
        self.crop = Some(crop);
        self
    }

    #[must_use]
    pub const fn actor_id(&self) -> UserId {
        self.actor_id
    }

    #[must_use]
    pub const fn source(&self) -> &Source {
        self.source
    }

    #[must_use]
    pub const fn source_identity(&self) -> &SourceIdentity {
        &self.source_identity
    }

    #[must_use]
    pub const fn operation_id(&self) -> AcquisitionOperationId {
        self.operation_id
    }

    #[must_use]
    pub const fn acquired_at(&self) -> Timestamp {
        self.acquired_at
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8] {
        self.bytes
    }

    #[must_use]
    pub const fn selected_crop(&self) -> Option<AvatarCrop> {
        self.crop
    }
}

/// Records produced by an avatar import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AvatarImportReceipt {
    original_artifact_id: AvatarArtifactId,
    thumbnail_artifact_id: ArtifactId,
    acquisition_id: AcquisitionId,
    original_dimensions: ImageDimensions,
}

impl AvatarImportReceipt {
    #[must_use]
    pub const fn original_artifact_id(&self) -> AvatarArtifactId {
        self.original_artifact_id
    }

    #[must_use]
    pub const fn thumbnail_artifact_id(&self) -> ArtifactId {
        self.thumbnail_artifact_id
    }

    #[must_use]
    pub const fn acquisition_id(&self) -> AcquisitionId {
        self.acquisition_id
    }

    #[must_use]
    pub const fn original_dimensions(&self) -> ImageDimensions {
        self.original_dimensions
    }
}

/// Profile-avatar import service.
pub struct AvatarImporter<'a> {
    storage: &'a Storage,
}

impl<'a> AvatarImporter<'a> {
    #[must_use]
    pub const fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    /// Validates, stores, and selects a profile avatar.
    ///
    /// Input format is detected from bytes. PNG, JPEG, and WebP are accepted; the exact original
    /// is retained and the selected crop, or a centered default, becomes a square PNG thumbnail.
    /// # Errors
    /// [`AvatarImportError`] without changing storage
    /// when validation or persistence fails.
    pub async fn import(
        &self,
        request: AvatarImportRequest<'_>,
    ) -> Result<AvatarImportReceipt, AvatarImportError> {
        if request.actor_id != request.source.owner_id() {
            return Err(AvatarImportError::ActorCannotUseSource);
        }
        let prepared = prepare_image(request.bytes, request.crop)?;
        let ids = AvatarImportIds::from_operation(request.operation_id);
        let original = Artifact::from_bytes(ids.original, prepared.media_type, request.bytes);
        let thumbnail_media_type = PNG_MEDIA_TYPE
            .parse::<MediaType>()
            .map_err(|error| definition("avatar thumbnail media type", error))?;
        let thumbnail = Artifact::from_bytes(
            ids.thumbnail,
            thumbnail_media_type,
            &prepared.thumbnail_bytes,
        );
        let acquisition = Acquisition::from_source(
            ids.acquisition,
            request.operation_id,
            original.id(),
            request.source,
            request.source_identity,
            request.acquired_at,
        );
        let avatar = ProfileAvatar::from_parts(
            &original,
            request.bytes,
            &acquisition,
            prepared.dimensions,
            &thumbnail,
            &prepared.thumbnail_bytes,
        )?;
        self.storage.save_profile_avatar(avatar).await?;
        Ok(AvatarImportReceipt {
            original_artifact_id: AvatarArtifactId::from_artifact_id(original.id()),
            thumbnail_artifact_id: thumbnail.id(),
            acquisition_id: acquisition.id(),
            original_dimensions: prepared.dimensions,
        })
    }

    /// Clears a user's selected avatar while retaining imported originals.
    /// # Errors
    /// [`AvatarImportError`] for persistence errors or an unknown user.
    pub async fn clear(&self, actor_id: UserId) -> Result<(), AvatarImportError> {
        self.storage.clear_profile_avatar(actor_id).await?;
        Ok(())
    }
}

struct PreparedImage {
    media_type: MediaType,
    dimensions: ImageDimensions,
    thumbnail_bytes: Vec<u8>,
}

/// Decoded, orientation-corrected image for crop presentation.
pub struct AvatarPreview {
    dimensions: ImageDimensions,
    rgba: Vec<u8>,
}

impl AvatarPreview {
    #[must_use]
    pub const fn dimensions(&self) -> ImageDimensions {
        self.dimensions
    }

    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// Decodes an avatar with the same validation used by import.
/// # Errors
/// [`AvatarImportError`] for unsupported, oversized, or invalid images.
pub fn avatar_preview(bytes: &[u8]) -> Result<AvatarPreview, AvatarImportError> {
    let decoded = decode_image(bytes)?;
    Ok(AvatarPreview {
        dimensions: decoded.dimensions,
        rgba: decoded.image.to_rgba8().into_raw(),
    })
}

fn prepare_image(
    bytes: &[u8],
    crop: Option<AvatarCrop>,
) -> Result<PreparedImage, AvatarImportError> {
    let decoded = decode_image(bytes)?;
    let thumbnail = if let Some(crop) = crop {
        let right = crop.left.checked_add(crop.edge);
        let bottom = crop.top.checked_add(crop.edge);
        if right.is_none_or(|right| right > decoded.image.width())
            || bottom.is_none_or(|bottom| bottom > decoded.image.height())
        {
            return Err(AvatarCropError::OutsideImage.into());
        }
        decoded
            .image
            .crop_imm(crop.left, crop.top, crop.edge, crop.edge)
            .resize_exact(THUMBNAIL_EDGE, THUMBNAIL_EDGE, FilterType::Lanczos3)
    } else {
        decoded
            .image
            .resize_to_fill(THUMBNAIL_EDGE, THUMBNAIL_EDGE, FilterType::Lanczos3)
    };
    let mut thumbnail_bytes = Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut thumbnail_bytes, ImageFormat::Png)
        .map_err(AvatarImportError::ThumbnailEncoding)?;
    Ok(PreparedImage {
        media_type: decoded.media_type,
        dimensions: decoded.dimensions,
        thumbnail_bytes: thumbnail_bytes.into_inner(),
    })
}

struct DecodedImage {
    media_type: MediaType,
    dimensions: ImageDimensions,
    image: DynamicImage,
}

fn decode_image(bytes: &[u8]) -> Result<DecodedImage, AvatarImportError> {
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err(AvatarImportError::EncodedImageTooLarge);
    }
    let format = image::guess_format(bytes).map_err(AvatarImportError::Decode)?;
    let media_type = match format {
        ImageFormat::Png => PNG_MEDIA_TYPE,
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        _ => return Err(AvatarImportError::UnsupportedFormat),
    }
    .parse::<MediaType>()
    .map_err(|error| definition("avatar media type", error))?;

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_AVATAR_DIMENSION);
    limits.max_image_height = Some(MAX_AVATAR_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(AvatarImportError::Decode)?;
    let orientation = decoder.orientation().map_err(AvatarImportError::Decode)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(AvatarImportError::Decode)?;
    image.apply_orientation(orientation);
    Ok(DecodedImage {
        media_type,
        dimensions: ImageDimensions::from_width_height(image.width(), image.height())?,
        image,
    })
}

#[derive(Clone, Copy)]
struct AvatarImportIds {
    original: ArtifactId,
    thumbnail: ArtifactId,
    acquisition: AcquisitionId,
}

impl AvatarImportIds {
    fn from_operation(operation: AcquisitionOperationId) -> Self {
        // Any thumbnail-byte change requires a new version because artifact IDs are immutable.
        let thumbnail = format!("thumbnail/{THUMBNAIL_EDGE}/png/v{THUMBNAIL_PIPELINE_VERSION}");
        Self {
            original: derived_id::<ArtifactIdKind>(operation, "original"),
            thumbnail: derived_id::<ArtifactIdKind>(operation, thumbnail),
            acquisition: derived_id::<AcquisitionIdKind>(operation, "acquisition"),
        }
    }
}

fn derived_id<K>(operation: AcquisitionOperationId, record: impl std::fmt::Display) -> TypedUuid<K>
where
    K: TypedUuidKind,
{
    // Retain this UUID v5 domain; persisted avatar IDs depend on it.
    let name = format!("https://github.com/kubijo/nimrag/avatar-import/{operation}/{record}");
    TypedUuid::from_untyped_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()))
}

fn definition(field: &'static str, error: impl std::fmt::Display) -> AvatarImportError {
    AvatarImportError::InvalidDefinition {
        field,
        reason: error.to_string(),
    }
}

/// Avatar import failure.
#[derive(Debug, Error)]
pub enum AvatarImportError {
    #[error("acting user cannot import through another user's source")]
    ActorCannotUseSource,
    #[error("avatar image exceeds the {MAX_AVATAR_BYTES}-byte limit")]
    EncodedImageTooLarge,
    #[error("avatar image must be PNG, JPEG, or WebP")]
    UnsupportedFormat,
    #[error("avatar image is invalid: {0}")]
    Decode(image::ImageError),
    #[error("avatar thumbnail encoding failed: {0}")]
    ThumbnailEncoding(image::ImageError),
    #[error(transparent)]
    Dimensions(#[from] ImageDimensionsError),
    #[error(transparent)]
    Crop(#[from] AvatarCropError),
    #[error("internal {field} is invalid: {reason}")]
    InvalidDefinition {
        /// Invalid definition.
        field: &'static str,
        /// Failure reason.
        reason: String,
    },
    #[error(transparent)]
    Persistence(#[from] garmin_storage::AvatarPersistenceError),
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use image::{GenericImageView, Rgba, RgbaImage};

    use super::*;

    #[test]
    fn derived_id_schema_has_a_golden_vector() {
        let operation = AcquisitionOperationId::from_u128(0x12345678_1234_5678_1234_567812345678);
        let ids = AvatarImportIds::from_operation(operation);

        assert_eq!(
            [
                ids.original.to_string(),
                ids.thumbnail.to_string(),
                ids.acquisition.to_string(),
            ],
            [
                "1225c1bf-f107-5665-9921-7408f1677768",
                "648c8e10-89a2-5113-99c3-4169575daa2d",
                "49d48489-a6e8-567c-b48b-4a05f32772c8",
            ]
        );
    }

    #[test]
    fn preview_and_explicit_crop_share_oriented_pixels() -> Result<(), Box<dyn Error>> {
        let mut source = RgbaImage::new(4, 2);
        for (x, _y, pixel) in source.enumerate_pixels_mut() {
            *pixel = if x < 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
        }
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source).write_to(&mut encoded, ImageFormat::Png)?;
        let encoded = encoded.into_inner();

        let preview = avatar_preview(&encoded)?;
        assert_eq!(preview.dimensions().into_width_height(), (4, 2));
        let crop = AvatarCrop::from_pixels(2, 0, 2)?;
        let prepared = prepare_image(&encoded, Some(crop))?;
        let thumbnail = image::load_from_memory(&prepared.thumbnail_bytes)?;

        assert_eq!(thumbnail.dimensions(), (THUMBNAIL_EDGE, THUMBNAIL_EDGE));
        assert_eq!(thumbnail.get_pixel(0, 0), Rgba([0, 0, 255, 255]));
        Ok(())
    }
}
