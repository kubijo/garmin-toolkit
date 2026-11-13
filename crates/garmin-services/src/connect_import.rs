use std::io::{Cursor, Read};

use garmin_model::{
    artifact::{AcquisitionOperationId, ByteCount, SourceIdentity},
    garmin_connect::{ActivityRequest, ActivityResponse},
    identity::{Source, SourceId},
    value::Timestamp,
};
use garmin_service_api::{ConnectActivityResult, ConnectFitPreview};
use uuid::Uuid;

use crate::{
    Application, Error, FitImportRequest, FitImportResult, ImportDisposition, UserContext,
};

const FIT_LIMIT: ByteCount = ByteCount::from_mebibytes(64);
const PREVIEW_LIMIT: ByteCount = ByteCount::from_mebibytes(8);

mod cache;
mod search;

impl From<Error> for garmin_model::garmin_connect::ActivityFailure {
    fn from(error: Error) -> Self {
        use garmin_connect::Error as ConnectError;
        use garmin_model::garmin_connect::ActivityFailureKind;
        let kind = match &error {
            Error::GarminConnect(
                ConnectError::Network
                | ConnectError::Timeout
                | ConnectError::InvalidToken
                | ConnectError::InvalidProfile
                | ConnectError::EmptyProfile
                | ConnectError::MissingAccountIdentity
                | ConnectError::Rejected {
                    status: 401 | 403 | 429,
                    ..
                },
            )
            | Error::GarminConnectKeyMissing
            | Error::ProfileNotFound(_)
            | Error::Storage(_) => ActivityFailureKind::Session,
            _ => ActivityFailureKind::Activity,
        };
        Self {
            message: error.to_string(),
            kind,
        }
    }
}

impl Application {
    /// Browses or previews cached cloud data; only explicit imports create activity observations.
    /// # Errors
    /// Returns redacted transport, archive validation, normalization or storage failures.
    pub async fn garmin_connect_activity(
        &self,
        user: UserContext,
        request: ActivityRequest,
    ) -> Result<ConnectActivityResult, Error> {
        self.garmin_connect_activity_with_progress(user, request, |_| {})
            .await
    }

    /// Performs an activity request while publishing coalescible cache-load progress.
    /// # Errors
    /// Returns the same bounded failures as [`Self::garmin_connect_activity`].
    pub async fn garmin_connect_activity_with_progress(
        &self,
        user: UserContext,
        request: ActivityRequest,
        progress: impl Fn(garmin_model::garmin_connect::LoadProgress) + Send + Sync,
    ) -> Result<ConnectActivityResult, Error> {
        self.garmin_connect_configured(user).await?;
        let key = self
            .storage
            .garmin_connect_key(user.user_id())
            .await?
            .ok_or(Error::GarminConnectKeyMissing)?;
        let lock = self
            .connect_cache_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(user.user_id())
            .or_default()
            .clone();
        if matches!(
            request,
            ActivityRequest::Load { .. } | ActivityRequest::List { .. }
        ) {
            progress(garmin_model::garmin_connect::LoadProgress::Waiting);
        }
        let _guard = lock.lock().await;
        match request {
            ActivityRequest::List { start, filters } => {
                self.cached_connect_page(user, &key, start, filters, false, &progress)
                    .await
            }
            ActivityRequest::Load { start, filters } => {
                self.cached_connect_page(user, &key, start, filters, true, &progress)
                    .await
            }
            ActivityRequest::Preview { id } => {
                let (download, details) = self.cached_connect_download(user, &key, id).await?;
                let (mut preview, operation) = prepare_preview(user, id, &download)?;
                preview.details = details;
                preview.imported = self
                    .storage
                    .imported_operation(user.user_id(), operation)
                    .await?;
                bounded_preview(preview)
            }
            ActivityRequest::Prepare { id } => {
                let (download, _) = self.cached_connect_download(user, &key, id).await?;
                let (preview, operation) = prepare_preview(user, id, &download)?;
                let imported = self
                    .storage
                    .imported_operation(user.user_id(), operation)
                    .await?;
                Ok(prepared(preview, imported))
            }
            ActivityRequest::Import { id, digest } => {
                let (download, _) = self.cached_connect_download(user, &key, id).await?;
                self.import_reviewed_connect_download(user, id, &digest, download)
                    .await
                    .map(ConnectActivityResult::State)
            }
        }
    }

    async fn import_reviewed_connect_download(
        &self,
        user: UserContext,
        id: i64,
        digest: &str,
        download: garmin_connect::Download,
    ) -> Result<ActivityResponse, Error> {
        let bytes = original_fit(&download.archive)?;
        if preview_digest(user, id, &download, &bytes)? != digest {
            return Err(Error::GarminConnectPreviewChanged);
        }
        let source_id = connect_source_id(user, &download.account)?;
        let source = Source::from_parts(
            SourceId::from_u128(source_id.as_u128()),
            user.user_id(),
            "Garmin Connect"
                .parse()
                .map_err(|_| Error::GarminConnectArchive)?,
            None,
        );
        let identity =
            SourceIdentity::from_string(id.to_string()).map_err(|_| Error::GarminConnectArchive)?;
        let operation = connect_operation(source_id, id, &bytes);
        let result = self
            .import_fit(FitImportRequest::from_parts(
                user,
                &source,
                identity,
                operation,
                Timestamp::from_jiff(jiff::Timestamp::now()),
                &bytes,
            ))
            .await?;
        match result {
            FitImportResult::Imported {
                observations,
                disposition,
            } => Ok(ActivityResponse::Imported {
                id,
                activities: observations.len() as u64,
                duplicate: disposition == ImportDisposition::Existing,
            }),
            FitImportResult::Rejected { failure, .. } => {
                Err(Error::GarminConnectImport(failure.to_string()))
            }
        }
    }
}

// Leave room for the RPC envelope below the browser transport's buffered-message threshold.
// A small, typed failure is returned instead of invoking Remoc's threaded large-item path.
fn bounded_preview(preview: ConnectFitPreview) -> Result<ConnectActivityResult, Error> {
    let result = ConnectActivityResult::Preview(preview);
    let limit = usize::try_from(PREVIEW_LIMIT.into_u64())
        .map_err(|_| Error::GarminConnectPreviewTooLarge)?;
    // Remoc's default Postbag codec requires a connection context. Use its wire configuration
    // directly here, without field omission, to obtain a conservative bound outside that context.
    postbag::serialize(
        postbag::cfg::Cfg::<true>::new().with_header(false),
        PreviewBudget { remaining: limit },
        &result,
    )
    .map_err(|_| Error::GarminConnectPreviewTooLarge)?;
    Ok(result)
}

struct PreviewBudget {
    remaining: usize,
}

impl std::io::Write for PreviewBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("preview response exceeds browser budget"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn prepared(preview: ConnectFitPreview, imported: bool) -> ConnectActivityResult {
    ConnectActivityResult::Prepared {
        id: preview.id,
        digest: preview.digest,
        imported,
    }
}

fn connect_source_id(
    user: UserContext,
    account: &garmin_model::garmin_connect::AccountFacts,
) -> Result<Uuid, Error> {
    let account = account
        .display_name
        .as_deref()
        .ok_or(garmin_connect::Error::MissingAccountIdentity)?;
    let key = format!("garmin-personal-api:v1:{}:{account}", user.user_id());
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_URL, key.as_bytes()))
}

fn connect_operation(source: Uuid, id: i64, bytes: &[u8]) -> AcquisitionOperationId {
    let mut hash = blake3::Hasher::new();
    hash.update(b"garmin-personal-api-fit:v1");
    hash.update(source.as_bytes());
    hash.update(&id.to_be_bytes());
    hash.update(bytes);
    AcquisitionOperationId::from_u128(
        Uuid::new_v5(&Uuid::NAMESPACE_URL, hash.finalize().as_bytes()).as_u128(),
    )
}

fn prepare_preview(
    user: UserContext,
    id: i64,
    download: &garmin_connect::Download,
) -> Result<(ConnectFitPreview, AcquisitionOperationId), Error> {
    let bytes = original_fit(&download.archive)?;
    let preview = crate::devices::fit_preview::inspect(format!("{id}.fit"), &bytes)
        .map_err(Error::GarminConnectImport)?;
    if !preview.supports_activity_import() {
        return Err(Error::GarminConnectArchive);
    }
    let digest = preview_digest(user, id, download, &bytes)?;
    let operation = connect_operation(connect_source_id(user, &download.account)?, id, &bytes);
    Ok((
        ConnectFitPreview {
            id,
            digest,
            details: garmin_model::garmin_connect::ActivityDetails {
                name: None,
                description: None,
            },
            imported: false,
            preview,
        },
        operation,
    ))
}

fn original_fit(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let invalid = || Error::GarminConnectArchive;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if zip.is_empty() || zip.len() > 16 {
        return Err(invalid());
    }
    let mut found = None;
    for index in 0..zip.len() {
        let file = zip.by_index(index).map_err(|_| invalid())?;
        if file.is_dir() {
            continue;
        }
        if file.enclosed_name().is_none()
            || !file
                .name()
                .map_err(|_| invalid())?
                .to_ascii_lowercase()
                .ends_with(".fit")
            || found.is_some()
            || file.size() > FIT_LIMIT.into_u64()
        {
            return Err(invalid());
        }
        let mut fit = Vec::new();
        file.take(FIT_LIMIT.into_u64() + 1)
            .read_to_end(&mut fit)
            .map_err(|_| invalid())?;
        if fit.len() as u64 > FIT_LIMIT.into_u64() {
            return Err(invalid());
        }
        found = Some(fit);
    }
    found.ok_or_else(invalid)
}

fn preview_digest(
    user: UserContext,
    id: i64,
    download: &garmin_connect::Download,
    bytes: &[u8],
) -> Result<String, Error> {
    let account = download
        .account
        .display_name
        .as_deref()
        .ok_or(garmin_connect::Error::MissingAccountIdentity)?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"garmin-connect-preview:v2");
    hash.update(user.user_id().to_string().as_bytes());
    hash.update(&id.to_be_bytes());
    hash.update(&(account.len() as u64).to_be_bytes());
    hash.update(account.as_bytes());
    hash.update(bytes);
    Ok(hash.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_recordings_return_a_small_failure_or_a_compact_import_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        use garmin_service_api::{ActivitySampleSnapshot, DeviceFitPreviewContent};
        let _codec_guard = remoc::codec::allow_outside();
        let limit = usize::try_from(PREVIEW_LIMIT.into_u64())?;
        assert!(limit + 1024 < garmin_service_api::control::transport_config().max_data_size);
        assert!(limit < remoc::rch::DEFAULT_MAX_ITEM_SIZE);
        let bytes = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling)?;
        let (mut preview, _) = prepare_preview(
            UserContext::new(garmin_model::identity::UserId::from_u128(1)),
            42,
            &download("account", &bytes),
        )?;
        let mut encoded = Vec::new();
        <remoc::codec::Default as remoc::codec::Codec>::serialize_active(
            &mut encoded,
            &ConnectActivityResult::Preview(preview.clone()),
        )?;
        let mut checked_encoding = Vec::new();
        postbag::serialize(
            postbag::cfg::Cfg::<true>::new().with_header(false),
            &mut checked_encoding,
            &ConnectActivityResult::Preview(preview.clone()),
        )?;
        assert_eq!(
            encoded, checked_encoding,
            "the size check must match the configured RPC codec"
        );
        assert!(bounded_preview(preview.clone()).is_ok());
        let sample = ActivitySampleSnapshot {
            timestamp: Timestamp::from_jiff("2026-11-12T12:00:00Z".parse()?),
            coordinate: None,
            elevation_meters: None,
            distance: None,
            speed: None,
            heart_rate: None,
            cadence: None,
            power: None,
            temperature_millicelsius: None,
        };
        let mut sample_bytes = Vec::new();
        <remoc::codec::Default as remoc::codec::Codec>::serialize_active(
            &mut sample_bytes,
            &sample,
        )?;
        let size = sample_bytes.len();
        let DeviceFitPreviewContent::Activities(activities) = &mut preview.preview.content else {
            panic!("activity fixture");
        };
        activities[0].recording.samples =
            vec![sample; usize::try_from(PREVIEW_LIMIT.into_u64())? / size + 1];
        // Even this recording can be prepared for import without sending its samples to the browser.
        let receipt = prepared(preview.clone(), false);
        let mut receipt_bytes = Vec::new();
        <remoc::codec::Default as remoc::codec::Codec>::serialize_active(
            &mut receipt_bytes,
            &receipt,
        )?;
        assert!(receipt_bytes.len() < 512);
        assert!(matches!(
            bounded_preview(preview),
            Err(Error::GarminConnectPreviewTooLarge)
        ));
        Ok(())
    }

    #[test]
    fn authentication_and_rate_limits_stop_page_imports_but_bad_files_do_not() {
        use garmin_model::garmin_connect::ActivityFailure;
        for status in [401, 403, 429] {
            let failure =
                ActivityFailure::from(Error::GarminConnect(garmin_connect::Error::Rejected {
                    operation: "activity download",
                    status,
                }));
            assert!(failure.stops_batch());
        }
        assert!(ActivityFailure::from(Error::GarminConnectKeyMissing).stops_batch());
        assert!(!ActivityFailure::from(Error::GarminConnectArchive).stops_batch());
        assert!(!ActivityFailure::from(Error::GarminConnectPreviewChanged).stops_batch());
    }
    use garmin_model::garmin_connect::AccountFacts;
    use std::io::Write;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            writer
                .start_file(
                    *name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn download(account: &str, bytes: &[u8]) -> garmin_connect::Download {
        garmin_connect::Download {
            account: AccountFacts {
                display_name: Some(account.into()),
                locale: None,
                measurement_system: None,
                time_zone: None,
            },
            archive: archive(&[("activity.fit", bytes)]),
        }
    }

    #[test]
    fn unsafe_ambiguous_and_non_fit_archives_are_refused() {
        for bytes in [
            b"not a ZIP".to_vec(),
            archive(&[("../activity.fit", b"fit")]),
            archive(&[("activity.gpx", b"gpx")]),
            archive(&[("one.fit", b"fit"), ("two.fit", b"fit")]),
        ] {
            assert!(original_fit(&bytes).is_err());
        }
    }

    #[test]
    fn preview_is_read_only_and_explicit_import_is_bound_to_bytes_and_deduplicated()
    -> Result<(), Box<dyn std::error::Error>> {
        futures_lite::future::block_on(async {
            let root = tempfile::tempdir()?;
            let application = Application::new(
                garmin_storage::Storage::open(root.path().join("cloud.sqlite3")).await?,
            );
            let owner = application.create_profile("Rider".parse()?).await?;
            let user = UserContext::new(owner.id());
            let bytes = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling)?;
            let (plan, operation) = prepare_preview(user, 42, &download("account-one", &bytes))?;
            assert!(
                !application
                    .storage
                    .imported_operation(user.user_id(), operation)
                    .await?
            );
            assert_eq!(plan.preview.activities().len(), 1);
            assert_eq!(application.activities(user).await?.len(), 0);
            assert!(matches!(
                application
                    .import_reviewed_connect_download(
                        user,
                        42,
                        &plan.digest,
                        download("account-two", &bytes)
                    )
                    .await,
                Err(Error::GarminConnectPreviewChanged)
            ));
            assert!(matches!(
                application
                    .import_reviewed_connect_download(
                        user,
                        42,
                        "changed-digest",
                        download("account-one", &bytes)
                    )
                    .await,
                Err(Error::GarminConnectPreviewChanged)
            ));
            assert_eq!(application.activities(user).await?.len(), 0);
            for duplicate in [false, true] {
                let result = application
                    .import_reviewed_connect_download(
                        user,
                        42,
                        &plan.digest,
                        download("account-one", &bytes),
                    )
                    .await?;
                assert!(
                    matches!(result, ActivityResponse::Imported { activities: 1, duplicate: actual, .. } if actual == duplicate)
                );
                assert_eq!(application.activities(user).await?.len(), 1);
            }
            let reopened = garmin_storage::Storage::open(root.path().join("cloud.sqlite3")).await?;
            assert!(
                reopened
                    .imported_operation(user.user_id(), operation)
                    .await?
            );
            let changed = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Running)?;
            let (_, changed_operation) =
                prepare_preview(user, 42, &download("account-one", &changed))?;
            assert!(
                !reopened
                    .imported_operation(user.user_id(), changed_operation)
                    .await?
            );
            // Query the same persisted database rather than relying on browser-session state.
            let source = connect_source_id(user, &download("account-one", &bytes).account)?;
            assert_eq!(
                reopened
                    .imported_source_identities(
                        user.user_id(),
                        SourceId::from_u128(source.as_u128())
                    )
                    .await?,
                ["42"]
            );
            let other_source = connect_source_id(user, &download("account-two", &bytes).account)?;
            assert_eq!(
                reopened
                    .imported_source_identities(
                        user.user_id(),
                        SourceId::from_u128(other_source.as_u128())
                    )
                    .await?,
                Vec::<String>::new()
            );
            Ok(())
        })
    }

    #[test]
    fn preview_cannot_be_retargeted_to_another_profile_or_activity()
    -> Result<(), Box<dyn std::error::Error>> {
        futures_lite::future::block_on(async {
            let root = tempfile::tempdir()?;
            let application = Application::new(
                garmin_storage::Storage::open(root.path().join("review.sqlite3")).await?,
            );
            let owner = application.create_profile("Rider".parse()?).await?;
            let user = UserContext::new(owner.id());
            let other = application.create_profile("Other rider".parse()?).await?;
            let other = UserContext::new(other.id());
            let bytes = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling)?;
            let (plan, _) = prepare_preview(user, 42, &download("account-one", &bytes))?;
            for (actor, id) in [(other, 42), (user, 43)] {
                assert!(matches!(
                    application
                        .import_reviewed_connect_download(
                            actor,
                            id,
                            &plan.digest,
                            download("account-one", &bytes)
                        )
                        .await,
                    Err(Error::GarminConnectPreviewChanged)
                ));
                assert_eq!(application.activities(actor).await?.len(), 0);
            }
            assert_eq!(application.activities(user).await?.len(), 0);
            Ok(())
        })
    }
}
