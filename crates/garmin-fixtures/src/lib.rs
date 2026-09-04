//! Publishable data for mock deployments and tests.

use std::{
    ffi::{OsStr, OsString},
    fmt::Display,
    fs,
    io::ErrorKind,
    path::Path,
    str::FromStr,
};

use garmin_fit::fixture::ActivityCase;
use garmin_importer::{FitImportOutcome, FitImportRequest, FitImporter, ImportReceipt};
use garmin_model::{
    artifact::{AcquisitionOperationId, SourceIdentity},
    identity::{Device, DeviceId, Profile, Role, Source, SourceId, User, UserId},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_storage::Storage;
use thiserror::Error;

/// Recreates and seeds a disposable mock database.
/// # Errors
/// [`SeedError`] when the old database cannot be removed or the new one cannot be seeded.
pub async fn recreate(path: impl AsRef<Path>) -> Result<Storage, SeedError> {
    let path = path.as_ref();
    remove_if_present(path)?;
    remove_if_present(sidecar(path.as_os_str(), "-wal"))?;
    remove_if_present(sidecar(path.as_os_str(), "-shm"))?;

    let storage = Storage::open(path).await?;
    seed(&storage).await?;
    Ok(storage)
}

fn sidecar(path: &OsStr, suffix: &str) -> OsString {
    let mut sidecar = path.to_owned();
    sidecar.push(suffix);
    sidecar
}

fn remove_if_present(path: impl AsRef<Path>) -> Result<(), SeedError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// One imported synthetic activity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededActivity {
    owner_id: UserId,
    case: ActivityCase,
    receipt: ImportReceipt,
}

impl SeededActivity {
    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn case(&self) -> ActivityCase {
        self.case
    }

    #[must_use]
    pub const fn receipt(&self) -> &ImportReceipt {
        &self.receipt
    }
}

/// Records produced by seeding the synthetic corpus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededCorpus {
    users: Vec<User>,
    activities: Vec<SeededActivity>,
}

impl SeededCorpus {
    #[must_use]
    pub fn users(&self) -> &[User] {
        &self.users
    }

    #[must_use]
    pub fn activities(&self) -> &[SeededActivity] {
        &self.activities
    }

    /// All produced observation IDs.
    pub fn observation_ids(&self) -> impl Iterator<Item = ObservationId> + '_ {
        self.activities
            .iter()
            .flat_map(|activity| activity.receipt.observation_ids().iter().copied())
    }
}

/// Seeds a database through the production FIT importer.
///
/// Stable IDs make repeated calls idempotent. No private or third-party bytes are used.
/// # Errors
/// [`SeedError`] when definitions, encoding, import, or persistence fail.
pub async fn seed(storage: &Storage) -> Result<SeededCorpus, SeedError> {
    let definitions = definitions()?;
    let mut users = Vec::with_capacity(definitions.len());
    let mut activities = Vec::with_capacity(ActivityCase::ALL.len());

    for definition in definitions {
        storage.save_user(&definition.user).await?;
        storage.save_device(&definition.device).await?;
        storage.save_source(&definition.source).await?;

        for &case in definition.activities {
            let bytes = case.encode()?;
            let request = FitImportRequest::from_parts(
                definition.user.id(),
                &definition.source,
                SourceIdentity::from_string(format!("GARMIN/ACTIVITY/{}", case.file_name()))
                    .map_err(|error| invalid("source identity", error))?,
                operation_id(case),
                Timestamp::from_unix_seconds(case.end())
                    .map_err(|error| invalid("acquisition time", error))?,
                &bytes,
            );
            let receipt = match FitImporter::new(storage).import(request).await? {
                FitImportOutcome::Imported(receipt) => receipt,
                FitImportOutcome::Rejected { failure, .. } => {
                    return Err(SeedError::Rejected {
                        file: case.file_name(),
                        reason: failure.to_string(),
                    });
                }
            };
            activities.push(SeededActivity {
                owner_id: definition.user.id(),
                case,
                receipt,
            });
        }
        users.push(definition.user);
    }

    Ok(SeededCorpus { users, activities })
}

struct Definition {
    user: User,
    device: Device,
    source: Source,
    activities: &'static [ActivityCase],
}

fn definitions() -> Result<[Definition; 3], SeedError> {
    Ok([
        definition(
            1,
            Role::Owner,
            "Alex Rider",
            "Synthetic fēnix",
            "Demo watch",
            &[ActivityCase::MorningRun, ActivityCase::TempoRun],
        )?,
        definition(
            2,
            Role::Member,
            "Sam Runner",
            "Synthetic running watch",
            "Demo running watch",
            &[ActivityCase::TrailRun],
        )?,
        definition(
            3,
            Role::Member,
            "Taylor Cyclist",
            "Synthetic Edge",
            "Demo bike computer",
            &[
                ActivityCase::CommuteRide,
                ActivityCase::EnduranceRide,
                ActivityCase::RecoveryRide,
            ],
        )?,
    ])
}

fn definition(
    ordinal: u128,
    role: Role,
    name: &str,
    device_label: &str,
    source_label: &str,
    activities: &'static [ActivityCase],
) -> Result<Definition, SeedError> {
    let user = User::from_parts(
        UserId::from_u128(0x1000_0000_0000_4000_8000_0000_0000_0000 + ordinal),
        role,
        Profile::from_display_name(parse("display name", name)?),
    );
    let device = Device::from_parts(
        DeviceId::from_u128(0x2000_0000_0000_4000_8000_0000_0000_0000 + ordinal),
        parse("device label", device_label)?,
    );
    let source = Source::from_parts(
        SourceId::from_u128(0x3000_0000_0000_4000_8000_0000_0000_0000 + ordinal),
        user.id(),
        parse("source label", source_label)?,
        Some(device.id()),
    );
    Ok(Definition {
        user,
        device,
        source,
        activities,
    })
}

const fn operation_id(case: ActivityCase) -> AcquisitionOperationId {
    let ordinal = match case {
        ActivityCase::MorningRun => 1,
        ActivityCase::TempoRun => 2,
        ActivityCase::TrailRun => 3,
        ActivityCase::CommuteRide => 4,
        ActivityCase::EnduranceRide => 5,
        ActivityCase::RecoveryRide => 6,
    };
    AcquisitionOperationId::from_u128(0x4000_0000_0000_4000_8000_0000_0000_0000 + ordinal)
}

fn parse<T>(field: &'static str, value: &str) -> Result<T, SeedError>
where
    T: FromStr,
    T::Err: Display,
{
    value.parse().map_err(|error| invalid(field, error))
}

fn invalid(field: &'static str, error: impl Display) -> SeedError {
    SeedError::InvalidDefinition {
        field,
        reason: error.to_string(),
    }
}

/// Development-corpus failure.
#[derive(Debug, Error)]
pub enum SeedError {
    #[error("could not reset mock storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid corpus {field}: {reason}")]
    InvalidDefinition {
        /// Invalid definition.
        field: &'static str,
        /// Failure reason.
        reason: String,
    },
    #[error(transparent)]
    Encoding(#[from] garmin_fit::fixture::EncodingError),
    #[error("generated FIT {file} was rejected: {reason}")]
    Rejected {
        /// Synthetic file name.
        file: &'static str,
        /// Parser failure.
        reason: String,
    },
    #[error(transparent)]
    Import(#[from] garmin_importer::ImportError),
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use futures_lite::future::block_on;
    use garmin_model::activity::ActivitySport;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn corpus_is_idempotent_and_preserves_every_fit_file() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("corpus.sqlite3")).await?;

            let first = seed(&storage).await?;
            let second = seed(&storage).await?;

            assert_eq!(first, second);
            assert_eq!(first.users().len(), 3);
            assert_eq!(first.activities().len(), ActivityCase::ALL.len());
            assert_eq!(first.observation_ids().count(), ActivityCase::ALL.len());

            for user in first.users() {
                assert_eq!(storage.user(user.id()).await?, Some(user.clone()));
            }
            for activity in first.activities() {
                assert_eq!(
                    storage
                        .artifact_bytes(activity.receipt().artifact_id())
                        .await?,
                    Some(activity.case().encode()?)
                );
                let [observation_id] = activity.receipt().observation_ids() else {
                    return Err("synthetic file did not produce one activity".into());
                };
                let stored = storage
                    .activity(activity.owner_id(), *observation_id)
                    .await?
                    .ok_or("seeded activity was missing")?;
                let (sport, distance) = expected(activity.case());
                assert_eq!(stored.normalized().activity().summary().sport(), sport);
                assert_eq!(
                    stored
                        .normalized()
                        .activity()
                        .summary()
                        .totals()
                        .distance()
                        .ok_or("synthetic activity had no distance")?
                        .as_millimeters(),
                    distance
                );
                assert_eq!(
                    stored
                        .normalized()
                        .activity()
                        .summary()
                        .time()
                        .start()
                        .as_unix_seconds(),
                    activity.case().start()
                );
            }

            let activity_counts = first
                .users()
                .iter()
                .map(|user| async { storage.activities(user.id()).await.map(|rows| rows.len()) });
            let mut counts = Vec::new();
            for count in activity_counts {
                counts.push(count.await?);
            }
            assert_eq!(counts, [2, 1, 3]);

            storage.close().await;
            Ok(())
        })
    }

    #[test]
    fn recreation_discards_previous_mock_state() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let path = root.path().join("corpus.sqlite3");
            let storage = Storage::open(&path).await?;
            let extra = definition(
                99,
                Role::Member,
                "Discarded profile",
                "Discarded device",
                "Discarded source",
                &[],
            )?;
            storage.save_user(&extra.user).await?;
            storage.close().await;

            let storage = recreate(&path).await?;
            let users = storage.users().await?;
            assert_eq!(users.len(), 3);
            storage.close().await;
            Ok(())
        })
    }

    const fn expected(case: ActivityCase) -> (ActivitySport, u64) {
        match case {
            ActivityCase::MorningRun => (ActivitySport::Running, 7_850_000),
            ActivityCase::TempoRun => (ActivitySport::Running, 10_870_000),
            ActivityCase::TrailRun => (ActivitySport::Running, 12_430_000),
            ActivityCase::CommuteRide => (ActivitySport::Cycling, 18_420_000),
            ActivityCase::EnduranceRide => (ActivitySport::Cycling, 67_420_000),
            ActivityCase::RecoveryRide => (ActivitySport::Cycling, 27_060_000),
        }
    }
}
