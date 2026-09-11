//! Transaction failures against the production MTP adapter and virtual responder.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use garmin_capture::SessionCapture;
use garmin_device::{
    BackupDestination, DeviceInventory, DevicePathStatus, DeviceStateSnapshot,
    MountedMtpBackupProgress, MountedMtpUploadProgress, SafeRelativePath,
    storage::{DeviceIoError, DeviceRead, DeviceWrite, MtpStorageDevice},
};
use garmin_model::map::MapAuthorization;
use garmin_progress::{
    OperationStage, ProgressEvent, ProgressEventKind, ProgressReporter, ProgressState,
};
use garmin_update::{
    DownloadProgress, DownloadSpec, MountedInstallError, MountedUpdateRecoveryOutcome, UpdatePlan,
    apply_mounted_mtp_with_progress, preflight_mounted_mtp_update, recover_mounted_mtp_update,
};
use md5::{Digest as _, Md5};
use mtp_rs::{
    VirtualDeviceConfig, VirtualStorageConfig, register_virtual_device, unregister_virtual_device,
};
use sha2::Sha256;
use tempfile::{TempDir, tempdir};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const ORIGINAL_MAP_BYTES: &[u8] = b"original map";
const CHANGED_MAP_BYTES: &[u8] = b"changed map!";
const REPLACEMENT_MAP_BYTES: &[u8] = b"new map payload, longer than the original map";
const SECOND_ORIGINAL_MAP_BYTES: &[u8] = b"second original";
const SECOND_REPLACEMENT_MAP_BYTES: &[u8] = b"second replacement payload";
const ADDED_MAP_BYTES: &[u8] = b"new optional map";
const AUTHORIZATION_BYTES: &[u8] = b"authorization payload";
const WORKER_ROOT_ENV: &str = "GARMIN_UPDATE_CRASH_WORKER_ROOT";
const WORKER_MODE_ENV: &str = "GARMIN_UPDATE_CRASH_WORKER_MODE";
const TEST_CAPACITY: u64 = 100_000_000;
const DEVICE_STATE_HEADROOM: u64 = 16 * 1024 * 1024;
const DEVICE_STATE_FIXTURE_ALLOWANCE: u64 = 8 * 1024;
const TEST_DEVICE_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn assert_boundary_storage_snapshots(events: &[ProgressEvent], storages: usize) {
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == ProgressEventKind::StorageSnapshot)
            .count(),
        storages * 2,
        "retain only initial and final capacity per storage"
    );
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    BackupFailure(usize),
    InspectFailure(usize),
    UploadFailure(usize),
    UploadAcceptanceFailure(usize),
    CancelAfterUpload(usize),
    RestoreFailure(usize),
    ChangedBeforeDelete,
    StateFailure,
    ProcessDeath(usize),
    RestoreDeath,
}

struct TestDevice {
    adapter: MtpStorageDevice,
    location: u64,
    fault: Fault,
    backups: AtomicUsize,
    inspections: AtomicUsize,
    restores: AtomicUsize,
    uploads: AtomicUsize,
    verifications: AtomicUsize,
    root: PathBuf,
    _temporary: Option<TempDir>,
}

impl TestDevice {
    fn new(fault: Fault, capacity: u64, read_only: bool) -> Result<Self> {
        let temporary = tempdir()?;
        let root = temporary.path().to_owned();
        Self::prepare(&root)?;
        Ok(Self::attach(
            root,
            fault,
            capacity,
            read_only,
            Some(temporary),
        ))
    }

    fn prepare(root: &Path) -> Result {
        let primary = root.join("internal");
        fs::create_dir_all(primary.join("Garmin"))?;
        fs::write(primary.join("Garmin/map.img"), ORIGINAL_MAP_BYTES)?;
        fs::write(primary.join("Garmin/untouched.img"), b"keep me")?;
        fs::create_dir_all(root.join("card"))?;
        Ok(())
    }

    fn attach(
        root: PathBuf,
        fault: Fault,
        capacity: u64,
        read_only: bool,
        temporary: Option<TempDir>,
    ) -> Self {
        let primary = root.join("internal");
        let card = root.join("card");
        let config = VirtualDeviceConfig {
            serial: root.display().to_string(),
            storages: vec![
                VirtualStorageConfig {
                    description: "Internal".to_owned(),
                    backing_dir: primary,
                    capacity,
                    read_only,
                },
                VirtualStorageConfig {
                    description: "Card".to_owned(),
                    backing_dir: card,
                    capacity: 1_000_000,
                    read_only: false,
                },
            ],
            event_poll_interval: Duration::ZERO,
            watch_backing_dirs: false,
            ..Default::default()
        };
        let location = register_virtual_device(&config).location_id;
        Self {
            adapter: MtpStorageDevice::new(
                location,
                Some("test-target".to_owned()),
                vec!["internal".to_owned(), "card".to_owned()],
                "internal".to_owned(),
            ),
            location,
            fault,
            backups: AtomicUsize::new(0),
            inspections: AtomicUsize::new(0),
            restores: AtomicUsize::new(0),
            uploads: AtomicUsize::new(0),
            verifications: AtomicUsize::new(0),
            root,
            _temporary: temporary,
        }
    }
}

impl Drop for TestDevice {
    fn drop(&mut self) {
        unregister_virtual_device(self.location);
    }
}

#[async_trait]
impl DeviceRead for TestDevice {
    fn execution_target(&self) -> Option<&str> {
        self.adapter.execution_target()
    }
    async fn state(&self) -> std::result::Result<DeviceStateSnapshot, DeviceIoError> {
        if matches!(self.fault, Fault::StateFailure) {
            return Err(std::io::Error::other("injected disconnect during storage query").into());
        }
        self.adapter.state().await
    }
    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> std::result::Result<DeviceInventory, DeviceIoError> {
        self.adapter.inventory(paths).await
    }
    async fn primary_storage_id(&self) -> std::result::Result<String, DeviceIoError> {
        self.adapter.primary_storage_id().await
    }
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> std::result::Result<DevicePathStatus, DeviceIoError> {
        let inspection = self.inspections.fetch_add(1, Ordering::Relaxed) + 1;
        if matches!(self.fault, Fault::InspectFailure(expected) if inspection == expected) {
            return Err(
                std::io::Error::other("injected disconnect during object inspection").into(),
            );
        }
        self.adapter.inspect(storage, path).await
    }
    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> std::result::Result<String, DeviceIoError> {
        let backup = self.backups.fetch_add(1, Ordering::Relaxed) + 1;
        if matches!(self.fault, Fault::BackupFailure(expected) if backup == expected) {
            return Err(std::io::Error::other("injected disconnect during backup").into());
        }
        self.adapter
            .backup(storage, path, size, destination, progress)
            .await
    }
    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> std::result::Result<(), DeviceIoError> {
        self.verifications.fetch_add(1, Ordering::Relaxed);
        self.adapter.verify(storage, path, size, sha256).await
    }
    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> std::result::Result<Option<Vec<u8>>, DeviceIoError> {
        self.adapter.read_bounded_file(storage, path, limit).await
    }
    async fn list_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> std::result::Result<Vec<garmin_device::DeviceDirectoryEntry>, DeviceIoError> {
        self.adapter.list_directory(storage, path).await
    }
}

#[async_trait]
impl DeviceWrite for TestDevice {
    async fn ensure_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> std::result::Result<(), DeviceIoError> {
        self.adapter.ensure_directory(storage, path).await
    }
    async fn create_verified_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> std::result::Result<(), DeviceIoError> {
        self.adapter
            .create_verified_file(storage, path, bytes)
            .await
    }
    async fn remove_empty_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> std::result::Result<(), DeviceIoError> {
        self.adapter.remove_empty_directory(storage, path).await
    }
    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> std::result::Result<(), DeviceIoError> {
        if matches!(self.fault, Fault::ChangedBeforeDelete)
            && path.to_string().eq_ignore_ascii_case("Garmin/map.img")
        {
            fs::write(
                self.root.join("internal").join(path.as_path()),
                CHANGED_MAP_BYTES,
            )?;
        }
        self.adapter.delete(storage, path, size, sha256).await
    }
    async fn delete_size_checked(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> std::result::Result<(), DeviceIoError> {
        if matches!(self.fault, Fault::ChangedBeforeDelete)
            && path.to_string().eq_ignore_ascii_case("Garmin/map.img")
        {
            fs::write(
                self.root.join("internal").join(path.as_path()),
                CHANGED_MAP_BYTES,
            )?;
        }
        self.adapter.delete_size_checked(storage, path, size).await
    }
    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> std::result::Result<(), DeviceIoError> {
        let restore = self.restores.fetch_add(1, Ordering::Relaxed) + 1;
        if matches!(self.fault, Fault::RestoreFailure(expected) if restore == expected) {
            return Err(std::io::Error::other("injected disconnect during restore").into());
        }
        if matches!(self.fault, Fault::RestoreDeath) {
            let bytes = fs::read(backup)?;
            let partial = &bytes[..5];
            let staging = tempfile::NamedTempFile::new()?;
            fs::write(staging.path(), partial)?;
            self.adapter
                .upload(
                    storage,
                    path,
                    staging.path(),
                    partial.len() as u64,
                    &digest(partial),
                    MountedMtpUploadProgress {
                        reporter: ProgressReporter::default(),
                        completed_before: 0,
                        total: partial.len() as u64,
                    },
                )
                .await?;
            return std::future::pending().await;
        }
        self.adapter
            .restore(storage, path, size, backup, sha256)
            .await
    }
    async fn upload(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        source: &Path,
        size: u64,
        sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> std::result::Result<(), DeviceIoError> {
        let upload = self.uploads.fetch_add(1, Ordering::Relaxed) + 1;
        let fault = match self.fault {
            Fault::None
            | Fault::BackupFailure(_)
            | Fault::InspectFailure(_)
            | Fault::UploadAcceptanceFailure(_)
            | Fault::CancelAfterUpload(_)
            | Fault::RestoreFailure(_)
            | Fault::ChangedBeforeDelete
            | Fault::StateFailure
            | Fault::RestoreDeath => false,
            Fault::UploadFailure(expected) | Fault::ProcessDeath(expected) => upload == expected,
        };
        if fault {
            let source_bytes = fs::read(source)?;
            let partial = &source_bytes[..source_bytes.len().saturating_sub(1).min(9)];
            if partial.is_empty() {
                return Err(std::io::Error::other("cannot inject an empty partial upload").into());
            }
            let staging = tempfile::NamedTempFile::new()?;
            fs::write(staging.path(), partial)?;
            self.adapter
                .upload(
                    storage,
                    path,
                    staging.path(),
                    partial.len() as u64,
                    &digest(partial),
                    progress,
                )
                .await?;
            match self.fault {
                Fault::UploadFailure(_) => {
                    Err(std::io::Error::other("injected disconnect after partial upload").into())
                }
                Fault::ProcessDeath(_) => std::future::pending().await,
                _ => unreachable!(),
            }
        } else {
            let cancellation = progress.reporter.cancellation_token();
            self.adapter
                .upload(storage, path, source, size, sha256, progress)
                .await?;
            match self.fault {
                Fault::UploadAcceptanceFailure(expected) if upload == expected => {
                    self.adapter.delete(storage, path, size, sha256).await?;
                    Err(DeviceIoError::Verification(path.to_string()))
                }
                Fault::CancelAfterUpload(expected) if upload == expected => {
                    cancellation.cancel();
                    Ok(())
                }
                _ => Ok(()),
            }
        }
    }
}

struct Transaction {
    plan: UpdatePlan,
    staged: Vec<DownloadProgress>,
    capture: SessionCapture,
    _temporary: Option<TempDir>,
}

impl Transaction {
    fn new() -> Result<Self> {
        let temporary = tempdir()?;
        let root = temporary.path().to_owned();
        Self::at(&root, Some(temporary))
    }

    fn at(root: &Path, temporary: Option<TempDir>) -> Result<Self> {
        fs::create_dir_all(root)?;
        let source = root.join("payload.bin");
        fs::write(&source, REPLACEMENT_MAP_BYTES)?;
        let spec = DownloadSpec {
            map_name: "Test map".to_owned(),
            source: "https://example.invalid/map.img".parse()?,
            alternate_sources: vec![],
            requires_garmin_token: false,
            destination: SafeRelativePath::parse("Garmin/map.img")?,
            cache_name: "payload.bin".to_owned(),
            size: REPLACEMENT_MAP_BYTES.len() as u64,
            md5: hex::encode(Md5::digest(REPLACEMENT_MAP_BYTES)),
        };
        Ok(Self {
            plan: UpdatePlan {
                schema_version: garmin_update::UPDATE_PLAN_SCHEMA_VERSION,
                device_digest: TEST_DEVICE_DIGEST.to_owned(),
                downloads: vec![spec],
                files_to_remove: vec![],
                identifiers: vec![],
                total_bytes: REPLACEMENT_MAP_BYTES.len() as u64,
                backup_policy: garmin_update::BackupPolicy::Verified,
                digest: String::new(),
            }
            .with_backup_policy(garmin_update::BackupPolicy::Verified)?,
            staged: vec![DownloadProgress {
                path: source,
                bytes: REPLACEMENT_MAP_BYTES.len() as u64,
                elapsed: Duration::ZERO,
                resumed_at: 0,
                from_cache: true,
            }],
            capture: SessionCapture::create(&root.join("capture"))?,
            _temporary: temporary,
        })
    }

    async fn apply(
        &self,
        device: &TestDevice,
    ) -> std::result::Result<garmin_update::ApplyReport, MountedInstallError> {
        self.apply_with(device, &MapAuthorization::default()).await
    }

    async fn apply_with(
        &self,
        device: &TestDevice,
        authorization: &MapAuthorization,
    ) -> std::result::Result<garmin_update::ApplyReport, MountedInstallError> {
        self.apply_with_progress(device, authorization, ProgressReporter::default())
            .await
    }

    async fn apply_with_progress(
        &self,
        device: &TestDevice,
        authorization: &MapAuthorization,
        progress: ProgressReporter,
    ) -> std::result::Result<garmin_update::ApplyReport, MountedInstallError> {
        apply_mounted_mtp_with_progress(
            &self.plan,
            &self.staged,
            authorization,
            device,
            &self.capture,
            progress,
        )
        .await
    }
}

fn multi_file_fixture(fault: Fault) -> Result<(TestDevice, Transaction, MapAuthorization)> {
    multi_file_fixture_with_capacity(fault, TEST_CAPACITY)
}

fn multi_file_fixture_with_capacity(
    fault: Fault,
    capacity: u64,
) -> Result<(TestDevice, Transaction, MapAuthorization)> {
    let device_root = tempdir()?;
    TestDevice::prepare(device_root.path())?;
    fs::write(
        device_root.path().join("internal/Garmin/map2.img"),
        SECOND_ORIGINAL_MAP_BYTES,
    )?;
    fs::write(
        device_root.path().join("internal/Garmin/obsolete.img"),
        b"obsolete",
    )?;
    let device = TestDevice::attach(
        device_root.path().to_owned(),
        fault,
        capacity,
        false,
        Some(device_root),
    );

    let transaction_root = tempdir()?;
    let downloads = [
        ("Garmin/map.img", REPLACEMENT_MAP_BYTES),
        ("Garmin/map2.img", SECOND_REPLACEMENT_MAP_BYTES),
        ("Garmin/new.img", ADDED_MAP_BYTES),
    ];
    let mut specs = Vec::new();
    let mut staged = Vec::new();
    for (index, (destination, bytes)) in downloads.into_iter().enumerate() {
        let source = transaction_root.path().join(format!("payload-{index}.bin"));
        fs::write(&source, bytes)?;
        specs.push(DownloadSpec {
            map_name: format!("Test map {index}"),
            source: format!("https://example.invalid/map-{index}.img").parse()?,
            alternate_sources: vec![],
            requires_garmin_token: false,
            destination: SafeRelativePath::parse(destination)?,
            cache_name: format!("payload-{index}.bin"),
            size: bytes.len() as u64,
            md5: hex::encode(Md5::digest(bytes)),
        });
        staged.push(DownloadProgress {
            path: source,
            bytes: bytes.len() as u64,
            elapsed: Duration::ZERO,
            resumed_at: 0,
            from_cache: true,
        });
    }
    let total_bytes = specs.iter().map(|spec| spec.size).sum();
    let transaction = Transaction {
        plan: UpdatePlan {
            schema_version: garmin_update::UPDATE_PLAN_SCHEMA_VERSION,
            device_digest: TEST_DEVICE_DIGEST.to_owned(),
            downloads: specs,
            files_to_remove: vec![SafeRelativePath::parse("Garmin/obsolete.img")?],
            identifiers: vec![],
            total_bytes,
            backup_policy: garmin_update::BackupPolicy::Verified,
            digest: String::new(),
        }
        .with_backup_policy(garmin_update::BackupPolicy::Verified)?,
        staged,
        capture: SessionCapture::create(&transaction_root.path().join("capture"))?,
        _temporary: Some(transaction_root),
    };
    let authorization = MapAuthorization {
        unlocks: vec![garmin_model::map::MapUnlock {
            file_name: "Garmin/unlock.gma".to_owned(),
            gma: "YXV0aG9yaXphdGlvbiBwYXlsb2Fk".to_owned(),
            codes: vec![],
        }],
        ..Default::default()
    };
    Ok((device, transaction, authorization))
}

async fn verify_path(device: &TestDevice, path: &str, bytes: &[u8]) -> Result {
    device
        .verify(
            "internal",
            &SafeRelativePath::parse(path)?,
            bytes.len() as u64,
            &digest(bytes),
        )
        .await?;
    Ok(())
}

async fn require_absent(device: &TestDevice, path: &str) -> Result {
    assert_eq!(
        device
            .inspect("internal", &SafeRelativePath::parse(path)?)
            .await?,
        DevicePathStatus::Missing
    );
    Ok(())
}

async fn occupy_marker(transaction: &Transaction, relative: &str) -> Result {
    transaction
        .capture
        .write_bytes(Path::new(relative), b"occupied")
        .await?;
    Ok(())
}

async fn verify_multi_file_originals(device: &TestDevice) -> Result {
    verify_path(device, "Garmin/map.img", ORIGINAL_MAP_BYTES).await?;
    verify_path(device, "Garmin/map2.img", SECOND_ORIGINAL_MAP_BYTES).await?;
    verify_path(device, "Garmin/obsolete.img", b"obsolete").await?;
    verify_path(device, "Garmin/untouched.img", b"keep me").await?;
    require_absent(device, "Garmin/new.img").await?;
    require_absent(device, "Garmin/unlock.gma").await?;
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn verify(device: &TestDevice, bytes: &[u8]) -> Result {
    device
        .verify(
            "internal",
            &SafeRelativePath::parse("Garmin/map.img")?,
            bytes.len() as u64,
            &digest(bytes),
        )
        .await?;
    device
        .verify(
            "internal",
            &SafeRelativePath::parse("Garmin/untouched.img")?,
            7,
            &digest(b"keep me"),
        )
        .await?;
    Ok(())
}

async fn recover(capture: &Path, device: &TestDevice) -> Result<MountedUpdateRecoveryOutcome> {
    Ok(recover_mounted_mtp_update(
        capture,
        TEST_DEVICE_DIGEST,
        device,
        &ProgressReporter::default(),
    )
    .await?
    .outcome)
}

#[tokio::test]
async fn capacity_rejects_before_mutation_despite_free_space_on_another_volume() -> Result {
    for (capacity, read_only) in [(20, false), (TEST_CAPACITY, true)] {
        let device = TestDevice::new(Fault::None, capacity, read_only)?;
        let tx = Transaction::new()?;
        assert!(matches!(
            tx.apply(&device).await,
            Err(MountedInstallError::Space(_))
        ));
        verify(&device, ORIGINAL_MAP_BYTES).await?;
        assert!(
            !tx.capture
                .root()
                .join("mounted-update/transaction")
                .exists()
        );
    }
    Ok(())
}

#[tokio::test]
async fn prepared_marker_collision_prevents_mutation() -> Result {
    let device = TestDevice::new(Fault::None, TEST_CAPACITY, false)?;
    let transaction = Transaction::new()?;
    occupy_marker(
        &transaction,
        "mounted-update/transaction/000000-prepared.json",
    )
    .await?;

    assert!(transaction.apply(&device).await.is_err());
    verify(&device, ORIGINAL_MAP_BYTES).await?;
    Ok(())
}

#[tokio::test]
async fn committed_marker_collision_rolls_back_device_mutations() -> Result {
    let device = TestDevice::new(Fault::None, TEST_CAPACITY, false)?;
    let transaction = Transaction::new()?;
    occupy_marker(&transaction, "mounted-update/transaction/committed.json").await?;

    assert!(transaction.apply(&device).await.is_err());
    verify(&device, ORIGINAL_MAP_BYTES).await?;
    assert!(recover(transaction.capture.root(), &device).await.is_err());
    Ok(())
}

#[tokio::test]
async fn rollback_marker_collision_preserves_restored_device_state() -> Result {
    let device = TestDevice::new(Fault::UploadFailure(1), TEST_CAPACITY, false)?;
    let transaction = Transaction::new()?;
    occupy_marker(&transaction, "mounted-update/transaction/rolled-back.json").await?;

    assert!(transaction.apply(&device).await.is_err());
    verify(&device, ORIGINAL_MAP_BYTES).await?;
    assert!(recover(transaction.capture.root(), &device).await.is_err());
    Ok(())
}

#[tokio::test]
async fn rollback_started_marker_collision_preserves_unproven_partial_state() -> Result {
    let device = TestDevice::new(Fault::UploadFailure(1), TEST_CAPACITY, false)?;
    let transaction = Transaction::new()?;
    occupy_marker(
        &transaction,
        "mounted-update/transaction/rollback-started.json",
    )
    .await?;

    assert!(transaction.apply(&device).await.is_err());
    assert_eq!(
        fs::metadata(device.root.join("internal/Garmin/map.img"))?.len(),
        9
    );
    verify_path(&device, "Garmin/untouched.img", b"keep me").await?;
    assert!(recover(transaction.capture.root(), &device).await.is_err());
    Ok(())
}

#[tokio::test]
async fn preflight_then_commit_preserves_backups_and_recovery_keeps_committed_files() -> Result {
    let device = TestDevice::new(Fault::None, TEST_CAPACITY, false)?;
    let tx = Transaction::new()?;
    preflight_mounted_mtp_update(
        &tx.plan,
        &tx.staged,
        &MapAuthorization::default(),
        &device,
        &tx.capture,
        &ProgressReporter::default(),
    )
    .await?;
    tx.apply(&device).await?;
    verify(&device, REPLACEMENT_MAP_BYTES).await?;
    let verifications_before_recovery = device.verifications.load(Ordering::Relaxed);
    assert_eq!(
        recover(tx.capture.root(), &device).await?,
        MountedUpdateRecoveryOutcome::Committed
    );
    assert_eq!(
        device.verifications.load(Ordering::Relaxed),
        verifications_before_recovery,
        "committed recovery must reconcile device metadata without content readback"
    );
    Ok(())
}

#[tokio::test]
async fn explicitly_skipped_backup_avoids_device_reads_and_records_the_policy() -> Result {
    let device = TestDevice::new(Fault::None, TEST_CAPACITY, false)?;
    let mut transaction = Transaction::new()?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;

    transaction.apply(&device).await?;

    verify(&device, REPLACEMENT_MAP_BYTES).await?;
    assert_eq!(device.backups.load(Ordering::Relaxed), 0);
    assert!(
        !transaction
            .capture
            .root()
            .join("mounted-update/backups")
            .exists()
    );
    let journal: serde_json::Value = serde_json::from_slice(&fs::read(
        transaction
            .capture
            .root()
            .join("mounted-update/transaction/committed.json"),
    )?)?;
    assert_eq!(journal["backup_policy"], "skip");
    assert_eq!(journal["writes"][0]["unbacked_original"]["size"], 12);
    Ok(())
}

#[tokio::test]
async fn skipped_backup_failure_resumes_from_retained_payload() -> Result {
    let device = TestDevice::new(Fault::UploadFailure(1), TEST_CAPACITY, false)?;
    let mut transaction = Transaction::new()?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;

    assert!(matches!(
        transaction.apply(&device).await,
        Err(MountedInstallError::UnprotectedMutation { .. })
    ));
    assert_eq!(device.backups.load(Ordering::Relaxed), 0);
    assert_eq!(
        recover_mounted_mtp_update(
            transaction.capture.root(),
            &transaction.plan.device_digest,
            &device,
            &ProgressReporter::default(),
        )
        .await?
        .outcome,
        MountedUpdateRecoveryOutcome::Resumed
    );
    verify(&device, REPLACEMENT_MAP_BYTES).await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_backup_free_recovery_preserves_pending_transaction_evidence() -> Result {
    let device = TestDevice::new(Fault::UploadFailure(1), TEST_CAPACITY, false)?;
    let mut transaction = Transaction::new()?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;
    assert!(transaction.apply(&device).await.is_err());
    let uploads_before = device.uploads.load(Ordering::Relaxed);

    let progress = ProgressReporter::default();
    let cancellation = progress.cancellation_token();
    let progress = progress.observe(move |event| {
        if event.stage == OperationStage::Verify && event.state == ProgressState::Advanced {
            cancellation.cancel();
        }
    });
    let recovery = recover_mounted_mtp_update(
        transaction.capture.root(),
        &transaction.plan.device_digest,
        &device,
        &progress,
    )
    .await;

    assert!(matches!(
        recovery,
        Err(MountedInstallError::UnprotectedMutation {
            operation,
            evidence: garmin_update::UnprotectedMutationEvidence::IntentRecorded {
                ..
            },
        }) if matches!(*operation, MountedInstallError::Cancelled)
    ));
    assert_eq!(device.uploads.load(Ordering::Relaxed), uploads_before);
    Ok(())
}

#[tokio::test]
async fn skipped_multi_file_failure_reuses_applied_writes_and_finishes_the_transaction() -> Result {
    let (device, mut transaction, authorization) = multi_file_fixture(Fault::UploadFailure(3))?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;

    let failure = transaction.apply_with(&device, &authorization).await;
    assert!(matches!(
        failure,
        Err(MountedInstallError::UnprotectedMutation {
            evidence: garmin_update::UnprotectedMutationEvidence::Applied {
                applied_operations: 2,
                total_operations: 5,
                ..
            },
            ..
        })
    ));

    let report = recover_mounted_mtp_update(
        transaction.capture.root(),
        &transaction.plan.device_digest,
        &device,
        &ProgressReporter::default(),
    )
    .await?;

    assert_eq!(report.outcome, MountedUpdateRecoveryOutcome::Resumed);
    assert_eq!(device.uploads.load(Ordering::Relaxed), 5);
    verify_path(&device, "Garmin/map.img", REPLACEMENT_MAP_BYTES).await?;
    verify_path(&device, "Garmin/map2.img", SECOND_REPLACEMENT_MAP_BYTES).await?;
    verify_path(&device, "Garmin/new.img", ADDED_MAP_BYTES).await?;
    verify_path(&device, "Garmin/unlock.gma", AUTHORIZATION_BYTES).await?;
    require_absent(&device, "Garmin/obsolete.img").await?;
    assert!(
        transaction
            .capture
            .root()
            .join("mounted-update/transaction/committed.json")
            .is_file()
    );
    Ok(())
}

#[tokio::test]
async fn backup_free_recovery_distinguishes_an_unstarted_same_size_replacement() -> Result {
    let (device, mut transaction, authorization) = multi_file_fixture(Fault::UploadFailure(1))?;
    let replacement = vec![b'x'; SECOND_ORIGINAL_MAP_BYTES.len()];
    fs::write(&transaction.staged[1].path, &replacement)?;
    transaction.staged[1].bytes = replacement.len() as u64;
    transaction.plan.downloads[1].size = replacement.len() as u64;
    transaction.plan.downloads[1].md5 = hex::encode(Md5::digest(&replacement));
    transaction.plan.total_bytes = transaction
        .plan
        .downloads
        .iter()
        .map(|item| item.size)
        .sum();
    transaction.plan = transaction
        .plan
        .with_backup_policy(garmin_update::BackupPolicy::Skip)?;

    assert!(
        transaction
            .apply_with(&device, &authorization)
            .await
            .is_err()
    );
    let report = recover_mounted_mtp_update(
        transaction.capture.root(),
        &transaction.plan.device_digest,
        &device,
        &ProgressReporter::default(),
    )
    .await?;

    assert_eq!(report.outcome, MountedUpdateRecoveryOutcome::Resumed);
    verify_path(&device, "Garmin/map2.img", &replacement).await?;
    Ok(())
}

#[tokio::test]
async fn backup_free_recovery_removes_a_partial_then_reclaims_space_before_uploading() -> Result {
    let initial_usage = (ORIGINAL_MAP_BYTES.len()
        + SECOND_ORIGINAL_MAP_BYTES.len()
        + b"obsolete".len()
        + b"keep me".len()) as u64;
    let peak_growth = (REPLACEMENT_MAP_BYTES.len() - ORIGINAL_MAP_BYTES.len()
        + SECOND_REPLACEMENT_MAP_BYTES.len()
        - SECOND_ORIGINAL_MAP_BYTES.len()
        + ADDED_MAP_BYTES.len()
        + AUTHORIZATION_BYTES.len()) as u64;
    let (device, mut transaction, authorization) = multi_file_fixture_with_capacity(
        Fault::UploadFailure(3),
        initial_usage + peak_growth + DEVICE_STATE_HEADROOM + DEVICE_STATE_FIXTURE_ALLOWANCE,
    )?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;
    let first_attempt = transaction.apply_with(&device, &authorization).await;
    assert!(first_attempt.is_err());
    let free = device
        .state()
        .await?
        .storages
        .into_iter()
        .find(|storage| storage.id == "internal")
        .and_then(|storage| storage.capacity.bytes().map(|(_, free)| free))
        .expect("internal test capacity");
    let external = vec![b'x'; usize::try_from(free - 20)?];
    fs::write(device.root.join("internal/Garmin/external.bin"), &external)?;

    let (progress, receiver) = ProgressReporter::channel();
    let report = recover_mounted_mtp_update(
        transaction.capture.root(),
        &transaction.plan.device_digest,
        &device,
        &progress,
    )
    .await?;
    let events = receiver.try_iter().collect::<Vec<_>>();

    assert_eq!(report.outcome, MountedUpdateRecoveryOutcome::Resumed);
    let storage_count = device.state().await?.storages.len();
    assert_boundary_storage_snapshots(&events, storage_count);
    assert!(events.iter().any(|event| {
        event.state == ProgressState::Started
            && event.label == "Removing proven incomplete update file"
            && event.path.as_deref() == Some("Garmin/new.img")
    }));
    assert!(events.iter().any(|event| {
        event.state == ProgressState::Started
            && event.label == "Removing superseded file to reclaim device space"
            && event.path.as_deref() == Some("Garmin/obsolete.img")
    }));
    assert!(events.iter().any(|event| {
        event.state == ProgressState::Completed
            && event
                .label
                .starts_with("Removed incomplete update file; reclaimed ")
            && event.label.contains("; free ")
            && event.label.contains(" → ")
            && event.path.as_deref() == Some("Garmin/new.img")
    }));
    assert!(events.iter().any(|event| {
        event.state == ProgressState::Completed
            && event
                .label
                .starts_with("Removed superseded file; reclaimed ")
            && event.label.contains("; free ")
            && event.label.contains(" → ")
            && event.path.as_deref() == Some("Garmin/obsolete.img")
    }));
    assert!(events.iter().any(|event| {
        event.stage == OperationStage::Inspect
            && event.state == ProgressState::Completed
            && event.label.starts_with("Storage snapshot — Internal:")
            && event.label.contains(" free of ")
    }));
    let partial_removed = events
        .iter()
        .position(|event| event.label == "Removing proven incomplete update file")
        .expect("partial write removal was reported");
    let applied_write_checked = events
        .iter()
        .position(|event| {
            event.label == "Existing device path and size match the journal"
                && event.path.as_deref() == Some("Garmin/map.img")
        })
        .expect("an already-applied write was checked against device metadata");
    let resumed_upload = events
        .iter()
        .position(|event| {
            event.stage == OperationStage::Commit
                && event.state == ProgressState::Advanced
                && event.path.as_deref() == Some("Garmin/new.img")
        })
        .expect("the missing recovery payload was uploaded");
    assert!(partial_removed < applied_write_checked);
    assert!(applied_write_checked < resumed_upload);
    verify_path(&device, "Garmin/new.img", ADDED_MAP_BYTES).await?;
    verify_path(&device, "Garmin/unlock.gma", AUTHORIZATION_BYTES).await?;
    require_absent(&device, "Garmin/obsolete.img").await?;
    verify_path(&device, "Garmin/external.bin", &external).await?;
    Ok(())
}

#[tokio::test]
async fn backup_free_recovery_preserves_an_applied_removal_that_reappears() -> Result {
    let (device, mut transaction, authorization) = multi_file_fixture(Fault::UploadFailure(3))?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;
    fs::write(
        device.root.join("internal/Garmin/obsolete-2.img"),
        b"obsolete two",
    )?;
    transaction
        .plan
        .files_to_remove
        .push(SafeRelativePath::parse("Garmin/obsolete-2.img")?);
    assert!(
        transaction
            .apply_with(&device, &authorization)
            .await
            .is_err()
    );

    let progress = ProgressReporter::default();
    let cancellation = progress.cancellation_token();
    let progress = progress.observe(move |event| {
        if event.state == ProgressState::Completed
            && event.label.starts_with("Removed obsolete file;")
            && event.path.as_deref() == Some("Garmin/obsolete.img")
        {
            cancellation.cancel();
        }
    });
    assert!(matches!(
        recover_mounted_mtp_update(
            transaction.capture.root(),
            &transaction.plan.device_digest,
            &device,
            &progress,
        )
        .await,
        Err(MountedInstallError::UnprotectedMutation { .. })
    ));
    require_absent(&device, "Garmin/obsolete.img").await?;
    verify_path(&device, "Garmin/obsolete-2.img", b"obsolete two").await?;

    fs::write(
        device.root.join("internal/Garmin/obsolete.img"),
        b"replaced",
    )?;
    assert!(matches!(
        recover_mounted_mtp_update(
            transaction.capture.root(),
            &transaction.plan.device_digest,
            &device,
            &ProgressReporter::default(),
        )
        .await,
        Err(MountedInstallError::UnprotectedMutation { operation, .. })
            if matches!(*operation, MountedInstallError::RecoveryEvidence(_))
    ));
    verify_path(&device, "Garmin/obsolete.img", b"replaced").await?;

    fs::remove_file(device.root.join("internal/Garmin/obsolete.img"))?;
    assert_eq!(
        recover_mounted_mtp_update(
            transaction.capture.root(),
            &transaction.plan.device_digest,
            &device,
            &ProgressReporter::default(),
        )
        .await?
        .outcome,
        MountedUpdateRecoveryOutcome::Resumed,
    );
    require_absent(&device, "Garmin/obsolete-2.img").await?;
    Ok(())
}

#[tokio::test]
async fn recovery_accepts_an_applied_file_by_path_and_size_without_readback() -> Result {
    let (device, mut transaction, authorization) = multi_file_fixture(Fault::UploadFailure(4))?;
    transaction.plan.backup_policy = garmin_update::BackupPolicy::Skip;
    assert!(
        transaction
            .apply_with(&device, &authorization)
            .await
            .is_err()
    );
    fs::remove_file(device.root.join("internal/Garmin/unlock.gma"))?;
    let changed = vec![b'x'; ADDED_MAP_BYTES.len()];
    fs::write(device.root.join("internal/Garmin/new.img"), &changed)?;
    let verifications_before_recovery = device.verifications.load(Ordering::Relaxed);
    let (progress, receiver) = ProgressReporter::channel();
    let report = recover_mounted_mtp_update(
        transaction.capture.root(),
        &transaction.plan.device_digest,
        &device,
        &progress,
    )
    .await?;
    let events = receiver.try_iter().collect::<Vec<_>>();

    assert_eq!(report.outcome, MountedUpdateRecoveryOutcome::Resumed);
    assert!(events.iter().any(|event| {
        event.state == ProgressState::Completed
            && event.label == "Existing device path and size match the journal"
            && event.path.as_deref() == Some("Garmin/new.img")
    }));
    assert_eq!(
        device.verifications.load(Ordering::Relaxed),
        verifications_before_recovery,
        "recovery must not read back an applied file whose path and size match"
    );
    assert_eq!(
        fs::read(device.root.join("internal/Garmin/new.img"))?,
        changed,
        "same-size contents are intentionally left for the Garmin device to validate"
    );
    Ok(())
}

#[tokio::test]
async fn multi_file_commit_covers_replacements_additions_authorization_and_removal() -> Result {
    let (device, transaction, authorization) = multi_file_fixture(Fault::None)?;
    let (progress, events) = ProgressReporter::channel();

    transaction
        .apply_with_progress(&device, &authorization, progress)
        .await?;
    let events = events.try_iter().collect::<Vec<_>>();
    let storage_count = device.state().await?.storages.len();
    assert_boundary_storage_snapshots(&events, storage_count);
    verify_path(&device, "Garmin/map.img", REPLACEMENT_MAP_BYTES).await?;
    verify_path(&device, "Garmin/map2.img", SECOND_REPLACEMENT_MAP_BYTES).await?;
    verify_path(&device, "Garmin/new.img", ADDED_MAP_BYTES).await?;
    verify_path(&device, "Garmin/unlock.gma", AUTHORIZATION_BYTES).await?;
    verify_path(&device, "Garmin/untouched.img", b"keep me").await?;
    require_absent(&device, "Garmin/obsolete.img").await?;
    assert_eq!(
        recover(transaction.capture.root(), &device).await?,
        MountedUpdateRecoveryOutcome::Committed
    );
    Ok(())
}

#[tokio::test]
async fn every_multi_file_upload_failure_restores_the_original_set() -> Result {
    for upload in 1..=4 {
        let (device, transaction, authorization) =
            multi_file_fixture(Fault::UploadFailure(upload))?;

        assert!(
            transaction
                .apply_with(&device, &authorization)
                .await
                .is_err()
        );
        verify_multi_file_originals(&device).await?;
        assert_eq!(
            recover(transaction.capture.root(), &device).await?,
            MountedUpdateRecoveryOutcome::AlreadyRolledBack
        );
    }
    Ok(())
}

#[tokio::test]
async fn every_multi_file_upload_acceptance_failure_restores_the_original_set() -> Result {
    for upload in 1..=4 {
        let (device, transaction, authorization) =
            multi_file_fixture(Fault::UploadAcceptanceFailure(upload))?;

        assert!(
            transaction
                .apply_with(&device, &authorization)
                .await
                .is_err()
        );
        verify_multi_file_originals(&device).await?;
    }
    Ok(())
}

#[tokio::test]
async fn cancellation_between_each_upload_rolls_back_the_transaction() -> Result {
    for upload in 1..=4 {
        let (device, transaction, authorization) =
            multi_file_fixture(Fault::CancelAfterUpload(upload))?;

        assert!(matches!(
            transaction.apply_with(&device, &authorization).await,
            Err(MountedInstallError::Cancelled)
        ));
        verify_multi_file_originals(&device).await?;
    }
    Ok(())
}

#[tokio::test]
async fn every_backup_disconnect_prevents_device_mutation() -> Result {
    let expected = [ORIGINAL_MAP_BYTES, SECOND_ORIGINAL_MAP_BYTES, b"obsolete"];
    for backup in 1..=3 {
        let (device, transaction, authorization) =
            multi_file_fixture(Fault::BackupFailure(backup))?;

        assert!(
            transaction
                .apply_with(&device, &authorization)
                .await
                .is_err()
        );
        verify_multi_file_originals(&device).await?;
        let root = transaction.capture.root().join("mounted-update");
        assert!(!root.join("transaction/000000-prepared.json").exists());
        for (index, bytes) in expected[..backup - 1].iter().enumerate() {
            assert_eq!(
                fs::read(root.join(format!("backups/{:06}.bin", index + 1)))?,
                *bytes
            );
        }
        assert_eq!(
            fs::metadata(root.join(format!("backups/{backup:06}.bin")))?.len(),
            0
        );
    }
    Ok(())
}

#[tokio::test]
async fn storage_disconnect_after_backup_prevents_device_mutation() -> Result {
    let (device, transaction, authorization) = multi_file_fixture(Fault::StateFailure)?;

    assert!(
        transaction
            .apply_with(&device, &authorization)
            .await
            .is_err()
    );
    verify_multi_file_originals(&device).await?;
    assert!(
        !transaction
            .capture
            .root()
            .join("mounted-update/transaction/000000-prepared.json")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn unsupported_authorization_is_rejected_before_mutation() -> Result {
    let cases = [
        MapAuthorization {
            embedded_unlocks: vec![garmin_model::map::EmbeddedMapUnlock::default()],
            ..Default::default()
        },
        MapAuthorization {
            signed_sd_card_bytes: Some("signed".to_owned()),
            ..Default::default()
        },
    ];
    for authorization in cases {
        let device = TestDevice::new(Fault::None, TEST_CAPACITY, false)?;
        let transaction = Transaction::new()?;

        assert!(
            transaction
                .apply_with(&device, &authorization)
                .await
                .is_err()
        );
        verify(&device, ORIGINAL_MAP_BYTES).await?;
        assert!(!transaction.capture.root().join("mounted-update").exists());
    }
    Ok(())
}

#[tokio::test]
async fn partial_upload_error_rolls_back_and_retry_verifies_originals() -> Result {
    let device = TestDevice::new(Fault::UploadFailure(1), TEST_CAPACITY, false)?;
    let tx = Transaction::new()?;
    assert!(tx.apply(&device).await.is_err());
    verify(&device, ORIGINAL_MAP_BYTES).await?;
    assert_eq!(
        recover(tx.capture.root(), &device).await?,
        MountedUpdateRecoveryOutcome::AlreadyRolledBack
    );
    Ok(())
}

async fn interrupted() -> Result<(Arc<TestDevice>, Arc<Transaction>, PathBuf)> {
    let device = Arc::new(TestDevice::new(
        Fault::ProcessDeath(1),
        TEST_CAPACITY,
        false,
    )?);
    let tx = Arc::new(Transaction::new()?);
    let worker_device = Arc::clone(&device);
    let worker_tx = Arc::clone(&tx);
    let worker = tokio::spawn(async move { worker_tx.apply(&worker_device).await });
    let partial = SafeRelativePath::parse("Garmin/map.img")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if device
                .inspect("internal", &partial)
                .await
                .is_ok_and(|status| status.size() == Some(9))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    worker.abort();
    assert!(worker.await.is_err_and(|error| error.is_cancelled()));
    let capture = tx.capture.root().to_owned();
    assert!(
        !capture
            .join("mounted-update/transaction/rolled-back.json")
            .exists()
    );
    Ok((device, tx, capture))
}

#[tokio::test]
async fn task_abort_restores_a_proven_partial_upload() -> Result {
    let (device, _tx, capture) = interrupted().await?;
    assert_eq!(
        recover(&capture, &device).await?,
        MountedUpdateRecoveryOutcome::Restored
    );
    verify(&device, ORIGINAL_MAP_BYTES).await?;
    assert_eq!(
        recover(&capture, &device).await?,
        MountedUpdateRecoveryOutcome::AlreadyRolledBack
    );
    fs::remove_file(device.root.join("internal/Garmin/map.img"))?;
    assert!(recover(&capture, &device).await.is_err());
    assert!(!device.root.join("internal/Garmin/map.img").exists());
    Ok(())
}

#[tokio::test]
async fn same_size_original_change_after_backup_does_not_force_a_second_device_read() -> Result {
    let device = TestDevice::new(Fault::ChangedBeforeDelete, TEST_CAPACITY, false)?;
    let transaction = Transaction::new()?;

    transaction.apply(&device).await?;
    let map = device.root.join("internal/Garmin/map.img");
    assert_eq!(fs::read(&map)?, REPLACEMENT_MAP_BYTES);
    assert_eq!(
        recover(transaction.capture.root(), &device).await?,
        MountedUpdateRecoveryOutcome::Committed
    );
    verify_path(&device, "Garmin/untouched.img", b"keep me").await?;
    Ok(())
}

#[tokio::test]
async fn recovery_inspection_disconnect_preserves_state_and_can_be_retried() -> Result {
    let (owner, _transaction, capture) = interrupted().await?;
    let map = owner.root.join("internal/Garmin/map.img");
    let partial = fs::read(&map)?;
    let disconnected = TestDevice::attach(
        owner.root.clone(),
        Fault::InspectFailure(1),
        TEST_CAPACITY,
        false,
        None,
    );

    assert!(recover(&capture, &disconnected).await.is_err());
    assert_eq!(fs::read(&map)?, partial);
    drop(disconnected);

    let reopened = TestDevice::attach(owner.root.clone(), Fault::None, TEST_CAPACITY, false, None);
    assert_eq!(
        recover(&capture, &reopened).await?,
        MountedUpdateRecoveryOutcome::Restored
    );
    verify(&reopened, ORIGINAL_MAP_BYTES).await?;
    Ok(())
}

#[tokio::test]
async fn recovery_restore_disconnect_can_be_reopened_and_retried() -> Result {
    let (owner, _transaction, capture) = interrupted().await?;
    let disconnected = TestDevice::attach(
        owner.root.clone(),
        Fault::RestoreFailure(1),
        TEST_CAPACITY,
        false,
        None,
    );

    assert!(recover(&capture, &disconnected).await.is_err());
    assert!(!owner.root.join("internal/Garmin/map.img").exists());
    drop(disconnected);

    let reopened = TestDevice::attach(owner.root.clone(), Fault::None, TEST_CAPACITY, false, None);
    assert_eq!(
        recover(&capture, &reopened).await?,
        MountedUpdateRecoveryOutcome::Restored
    );
    verify(&reopened, ORIGINAL_MAP_BYTES).await?;
    Ok(())
}

#[tokio::test]
async fn wrong_device_recovery_is_rejected_without_mutation() -> Result {
    let (device, _transaction, capture) = interrupted().await?;
    let partial = device.root.join("internal/Garmin/map.img");
    let before = fs::read(&partial)?;

    assert!(matches!(
        recover_mounted_mtp_update(
            &capture,
            "another-device",
            device.as_ref(),
            &ProgressReporter::default(),
        )
        .await,
        Err(MountedInstallError::RecoveryDeviceMismatch)
    ));
    assert_eq!(fs::read(&partial)?, before);
    Ok(())
}

#[tokio::test]
async fn task_abort_preserves_changed_partial_files_and_corrupted_backups() -> Result {
    for corrupt_backup in [false, true] {
        let (device, _tx, capture) = interrupted().await?;
        if corrupt_backup {
            let backup = fs::read_dir(capture.join("mounted-update/backups"))?
                .next()
                .ok_or("backup missing")??;
            fs::write(backup.path(), b"bad backup!!")?;
        } else {
            fs::write(device.root.join("internal/Garmin/map.img"), b"unrelated")?;
        }
        let expected = fs::read(device.root.join("internal/Garmin/map.img"))?;
        assert!(recover(&capture, &device).await.is_err());
        verify(&device, &expected).await?;
        assert!(
            !capture
                .join("mounted-update/transaction/rolled-back.json")
                .exists()
        );
    }
    Ok(())
}

#[tokio::test]
async fn task_abort_refuses_an_empty_partial_file() -> Result {
    let (device, _transaction, capture) = interrupted().await?;
    let partial = device.root.join("internal/Garmin/map.img");
    fs::write(&partial, [])?;

    assert!(recover(&capture, &device).await.is_err());
    assert_eq!(fs::metadata(&partial)?.len(), 0);
    assert!(
        !capture
            .join("mounted-update/transaction/rolled-back.json")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn process_crash_worker() -> Result {
    let Some(root) = std::env::var_os(WORKER_ROOT_ENV).map(PathBuf::from) else {
        return Ok(());
    };
    let recovery = std::env::var_os(WORKER_MODE_ENV).is_some_and(|mode| mode == "recover");
    let device = TestDevice::attach(
        root.join("device"),
        if recovery {
            Fault::RestoreDeath
        } else {
            Fault::ProcessDeath(1)
        },
        TEST_CAPACITY,
        false,
        None,
    );
    if recovery {
        recover_mounted_mtp_update(
            &root.join("transaction/capture"),
            TEST_DEVICE_DIGEST,
            &device,
            &ProgressReporter::default(),
        )
        .await?;
    } else {
        let transaction = Transaction::at(&root.join("transaction"), None)?;
        transaction.apply(&device).await?;
    }
    Err("crash worker unexpectedly completed".into())
}

async fn terminate_worker(
    root: &Path,
    mode: &str,
    partial: &Path,
    partial_size: u64,
    marker: &Path,
) -> Result {
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", "process_crash_worker", "--nocapture"])
        .env(WORKER_ROOT_ENV, root)
        .env(WORKER_MODE_ENV, mode)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::metadata(partial).is_ok_and(|metadata| metadata.len() == partial_size)
            && marker.is_file()
        {
            break;
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("crash worker exited before interruption: {status}").into());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            return Err("crash worker did not reach the injected interruption".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    child.kill()?;
    assert!(!child.wait()?.success());
    Ok(())
}

#[tokio::test]
async fn process_termination_during_commit_and_rollback_is_recoverable() -> Result {
    let root = tempdir()?;
    let device_root = root.path().join("device");
    TestDevice::prepare(&device_root)?;
    let partial = device_root.join("internal/Garmin/map.img");
    let prepared = root
        .path()
        .join("transaction/capture/mounted-update/transaction/000000-prepared.json");
    terminate_worker(root.path(), "apply", &partial, 9, &prepared).await?;

    let capture = root.path().join("transaction/capture");
    let constrained = TestDevice::attach(device_root.clone(), Fault::None, 16, false, None);
    assert!(matches!(
        recover_mounted_mtp_update(
            &capture,
            TEST_DEVICE_DIGEST,
            &constrained,
            &ProgressReporter::default(),
        )
        .await,
        Err(MountedInstallError::Space(_))
    ));
    assert_eq!(fs::metadata(&partial)?.len(), 9);
    drop(constrained);

    let rollback_started = root
        .path()
        .join("transaction/capture/mounted-update/transaction/rollback-started.json");
    terminate_worker(root.path(), "recover", &partial, 5, &rollback_started).await?;

    let device = TestDevice::attach(device_root, Fault::None, TEST_CAPACITY, false, None);
    assert_eq!(
        recover(&capture, &device).await?,
        MountedUpdateRecoveryOutcome::Restored
    );
    verify(&device, ORIGINAL_MAP_BYTES).await?;
    Ok(())
}
