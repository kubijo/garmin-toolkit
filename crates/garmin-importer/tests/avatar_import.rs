//! Profile-avatar import boundaries.

use std::{error::Error, io::Cursor};

use futures_lite::future::block_on;
use garmin_importer::{
    AvatarImportError, AvatarImportRequest, AvatarImporter, MAX_AVATAR_BYTES, THUMBNAIL_EDGE,
};
use garmin_model::{
    artifact::{AcquisitionOperationId, SourceIdentity},
    identity::{Profile, Role, Source, SourceId, User, UserId},
    value::Timestamp,
};
use garmin_storage::Storage;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct Fixture {
    user: User,
    source: Source,
}

impl Fixture {
    async fn stored(storage: &Storage) -> TestResult<Self> {
        let user = User::from_parts(
            UserId::new_v4(),
            Role::Owner,
            Profile::from_display_name("Rider".parse()?),
        );
        let source = Source::from_parts(
            SourceId::new_v4(),
            user.id(),
            "Profile image picker".parse()?,
            None,
        );
        storage.save_user(&user).await?;
        storage.save_source(&source).await?;
        Ok(Self { user, source })
    }

    fn request<'a>(
        &'a self,
        operation_id: AcquisitionOperationId,
        bytes: &'a [u8],
    ) -> TestResult<AvatarImportRequest<'a>> {
        Ok(AvatarImportRequest::from_parts(
            self.user.id(),
            &self.source,
            SourceIdentity::from_string(format!("profile-avatar/{operation_id}"))?,
            operation_id,
            "2026-09-01T12:00:00Z".parse::<Timestamp>()?,
            bytes,
        ))
    }
}

fn encoded_image(format: ImageFormat, width: u32, height: u32) -> TestResult<Vec<u8>> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        let red = u8::try_from(x % 256).expect("the modulo result fits u8");
        let green = u8::try_from(y % 256).expect("the modulo result fits u8");
        Rgb([red, green, 0x80])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image).write_to(&mut bytes, format)?;
    Ok(bytes.into_inner())
}

#[test]
fn imports_supported_images_and_round_trips_after_reopen() -> TestResult {
    block_on(async {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let root = tempdir()?;
            let path = root.path().join("avatar.sqlite3");
            let storage = Storage::open(&path).await?;
            let fixture = Fixture::stored(&storage).await?;
            let bytes = encoded_image(format, 640, 320)?;
            let operation = AcquisitionOperationId::new_v4();
            let importer = AvatarImporter::new(&storage);

            let receipt = importer.import(fixture.request(operation, &bytes)?).await?;
            assert_eq!(
                storage
                    .artifact_bytes(receipt.original_artifact_id().as_artifact_id())
                    .await?,
                Some(bytes.clone())
            );
            assert_eq!(
                receipt.original_dimensions().into_width_height(),
                (640, 320)
            );
            let selected = storage
                .profile_avatar(fixture.user.id())
                .await?
                .expect("the imported avatar is selected");
            assert_eq!(
                selected.original_artifact_id(),
                receipt.original_artifact_id()
            );
            assert_eq!(
                selected.thumbnail_artifact_id(),
                receipt.thumbnail_artifact_id()
            );
            let thumbnail =
                image::load_from_memory_with_format(selected.thumbnail_bytes(), ImageFormat::Png)?;
            assert_eq!(
                (thumbnail.width(), thumbnail.height()),
                (THUMBNAIL_EDGE, THUMBNAIL_EDGE)
            );
            let persisted_user = storage
                .user(fixture.user.id())
                .await?
                .expect("the avatar owner remains stored");
            assert_eq!(
                persisted_user.profile().avatar_artifact_id(),
                Some(receipt.original_artifact_id())
            );
            storage.close().await;

            let reopened = Storage::open(&path).await?;
            assert_eq!(
                reopened
                    .artifact_bytes(receipt.original_artifact_id().as_artifact_id())
                    .await?,
                Some(bytes)
            );
            assert!(reopened.profile_avatar(fixture.user.id()).await?.is_some());
            reopened.close().await;
        }
        Ok(())
    })
}

#[test]
fn retry_replacement_and_removal_preserve_immutable_originals() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("avatar.sqlite3")).await?;
        let fixture = Fixture::stored(&storage).await?;
        let importer = AvatarImporter::new(&storage);
        let first_bytes = encoded_image(ImageFormat::Png, 300, 200)?;
        let first_operation = AcquisitionOperationId::new_v4();
        let first = importer
            .import(fixture.request(first_operation, &first_bytes)?)
            .await?;
        assert_eq!(
            importer
                .import(fixture.request(first_operation, &first_bytes)?)
                .await?,
            first
        );

        let conflicting_bytes = encoded_image(ImageFormat::Png, 301, 200)?;
        assert!(
            importer
                .import(fixture.request(first_operation, &conflicting_bytes)?)
                .await
                .is_err()
        );
        assert_eq!(
            storage
                .profile_avatar(fixture.user.id())
                .await?
                .expect("the first avatar remains selected after conflict")
                .original_artifact_id(),
            first.original_artifact_id()
        );

        let second_bytes = encoded_image(ImageFormat::Jpeg, 200, 300)?;
        let second = importer
            .import(fixture.request(AcquisitionOperationId::new_v4(), &second_bytes)?)
            .await?;
        assert_eq!(
            storage
                .profile_avatar(fixture.user.id())
                .await?
                .expect("the replacement avatar is selected")
                .original_artifact_id(),
            second.original_artifact_id()
        );
        assert_eq!(
            storage
                .artifact_bytes(first.original_artifact_id().as_artifact_id())
                .await?,
            Some(first_bytes)
        );

        importer.clear(fixture.user.id()).await?;
        assert!(storage.profile_avatar(fixture.user.id()).await?.is_none());
        assert_eq!(
            storage
                .artifact_bytes(second.original_artifact_id().as_artifact_id())
                .await?,
            Some(second_bytes)
        );
        assert_eq!(
            storage
                .user(fixture.user.id())
                .await?
                .expect("the owner remains stored")
                .profile()
                .avatar_artifact_id(),
            None
        );
        storage.close().await;
        Ok(())
    })
}

#[test]
fn rejects_invalid_input_and_foreign_sources_without_mutation() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("avatar.sqlite3")).await?;
        let fixture = Fixture::stored(&storage).await?;
        let importer = AvatarImporter::new(&storage);
        let operation = AcquisitionOperationId::new_v4();

        let oversized = vec![0_u8; MAX_AVATAR_BYTES + 1];
        assert!(matches!(
            importer
                .import(fixture.request(operation, &oversized)?)
                .await,
            Err(AvatarImportError::EncodedImageTooLarge)
        ));
        assert!(
            importer
                .import(fixture.request(operation, b"not an image")?)
                .await
                .is_err()
        );
        assert!(matches!(
            importer
                .import(fixture.request(operation, b"GIF89a")?)
                .await,
            Err(AvatarImportError::UnsupportedFormat)
        ));
        let too_wide = encoded_image(ImageFormat::Png, 4_097, 1)?;
        assert!(
            importer
                .import(fixture.request(operation, &too_wide)?)
                .await
                .is_err()
        );

        let other_actor = UserId::new_v4();
        let bytes = encoded_image(ImageFormat::Png, 64, 64)?;
        let foreign_request = AvatarImportRequest::from_parts(
            other_actor,
            &fixture.source,
            "foreign/avatar".parse()?,
            operation,
            "2026-09-01T12:00:00Z".parse()?,
            &bytes,
        );
        assert!(matches!(
            importer.import(foreign_request).await,
            Err(AvatarImportError::ActorCannotUseSource)
        ));
        assert!(storage.profile_avatar(fixture.user.id()).await?.is_none());
        storage.close().await;
        Ok(())
    })
}
