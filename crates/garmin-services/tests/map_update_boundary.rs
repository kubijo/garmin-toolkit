use std::{
    path::Path,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use async_trait::async_trait;
use garmin_capture::SessionCapture;
use garmin_device::{
    BackupDestination, DeviceInventory, DevicePathStatus, DeviceStateSnapshot,
    MountedMtpBackupProgress, MountedMtpUploadProgress, SafeRelativePath, TransportKind,
    parse_manifest,
    storage::{DeviceDirectoryEntry, DeviceIoError, DeviceRead, DeviceWrite, DirectoryDevice},
};
use garmin_map_service::{ClientIdentity, OmtClient};
use garmin_progress::{OperationStage, ProgressReporter, ProgressState};
use garmin_services::maps::{
    GarminDownloadAuthorizer, PhysicalTarget, RecoveryExecution, SimulatedTarget, UpdateExecution,
    execute_registered_update, execute_update_plan, pending_recovery::PendingRecoveryStore,
    recover_update, recovery::PendingState,
};
use garmin_simulator::{MockAuthorization, MockServer};
use garmin_update::UpdatePlan;
use tempfile::TempDir;

const OLD: &[u8] = b"old mock content\n";

struct Fixture {
    root: TempDir,
    device: PathBuf,
    cache: PathBuf,
    capture: SessionCapture,
    client: OmtClient,
    manifest: garmin_device::DeviceManifest,
    plan: UpdatePlan,
    server: MockServer,
}

impl Fixture {
    async fn new(authorization: MockAuthorization) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let device = root.path().join("device");
        let cache = root.path().join("cache");
        let capture = SessionCapture::create(&root.path().join("capture"))?;
        garmin_simulator::create_fixture(&device).await?;
        let xml = tokio::fs::read_to_string(device.join("Garmin/GarminDevice.xml")).await?;
        let manifest = parse_manifest(
            &xml,
            TransportKind::MassStorage,
            device.display().to_string(),
        )?;
        let server = MockServer::start_with_authorization(authorization).await?;
        let client = OmtClient::local_mock(&ClientIdentity::default(), server.base_url().clone())?;
        let catalog = client
            .check_maps(
                manifest.raw_xml(),
                manifest.capabilities().installed_map_files(),
            )
            .await?;
        let selected = 0..catalog.maps.len() + catalog.bundled_maps.len();
        let plan = UpdatePlan::from_mock_response_selection(
            &catalog,
            manifest.identity_digest(),
            selected,
            server.base_url(),
        )?;
        garmin_simulator::seed_download_cache(&cache, &plan).await?;
        Ok(Self {
            root,
            device,
            cache,
            capture,
            client,
            manifest,
            plan,
            server,
        })
    }

    fn execution(&self, device: Box<dyn garmin_services::maps::UpdateTarget>) -> UpdateExecution {
        UpdateExecution {
            prepared: None,
            client: self.client.clone(),
            manifest: self.manifest.clone(),
            plan: self.plan.clone(),
            cache: self.cache.clone(),
            concurrency: 4,
            download_authorizer: std::sync::Arc::new(GarminDownloadAuthorizer),
            progress: ProgressReporter::default(),
            capture: self.capture.clone(),
            device,
        }
    }

    fn old_path(&self, name: &str) -> PathBuf {
        self.device.join("Garmin/Mock").join(name)
    }

    fn new_path(&self, name: &str) -> PathBuf {
        self.device.join("Garmin/Mock").join(name)
    }
}

#[derive(Clone, Copy)]
enum Fault {
    ChangedBeforeDelete,
    DisconnectDuringBackup,
    Upload,
    UploadAcceptance,
    Restore,
    ClearTransaction,
}

struct FaultDevice {
    inner: DirectoryDevice,
    fault: Fault,
    triggered: AtomicBool,
    path: PathBuf,
}

impl FaultDevice {
    fn new(root: &Path, fault: Fault) -> Self {
        Self {
            inner: DirectoryDevice::new(root.to_owned()),
            fault,
            triggered: AtomicBool::new(false),
            path: root.join("Garmin/Mock/europe.img"),
        }
    }
}

#[async_trait]
impl DeviceRead for FaultDevice {
    fn execution_target(&self) -> Option<&str> {
        self.inner.execution_target()
    }

    async fn state(&self) -> Result<DeviceStateSnapshot, DeviceIoError> {
        self.inner.state().await
    }

    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, DeviceIoError> {
        self.inner.inventory(paths).await
    }

    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        self.inner.primary_storage_id().await
    }

    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        self.inner.inspect(storage, path).await
    }

    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        if matches!(self.fault, Fault::DisconnectDuringBackup)
            && !self.triggered.swap(true, Ordering::SeqCst)
        {
            return Err(DeviceIoError::Transport(
                "injected disconnect during backup".to_owned(),
            ));
        }
        self.inner
            .backup(storage, path, size, destination, progress)
            .await
    }

    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        self.inner.verify(storage, path, size, sha256).await
    }

    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        self.inner.read_bounded_file(storage, path, limit).await
    }

    async fn list_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<DeviceDirectoryEntry>, DeviceIoError> {
        self.inner.list_directory(storage, path).await
    }
}

#[async_trait]
impl DeviceWrite for FaultDevice {
    async fn ensure_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        self.inner.ensure_directory(storage, path).await
    }

    async fn create_verified_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceIoError> {
        self.inner.create_verified_file(storage, path, bytes).await
    }

    async fn remove_empty_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        self.inner.remove_empty_directory(storage, path).await
    }

    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        if matches!(self.fault, Fault::ClearTransaction)
            && path
                .to_string()
                .eq_ignore_ascii_case("GARMIN-TOOLKIT/active.json")
        {
            return Err(DeviceIoError::Transport(
                "injected transaction cleanup failure".to_owned(),
            ));
        }
        if matches!(self.fault, Fault::ChangedBeforeDelete)
            && path
                .to_string()
                .eq_ignore_ascii_case("Garmin/Mock/europe.img")
            && !self.triggered.swap(true, Ordering::SeqCst)
        {
            tokio::fs::write(&self.path, b"changed outside the transaction").await?;
        }
        self.inner.delete(storage, path, size, sha256).await
    }

    async fn delete_size_checked(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        if matches!(self.fault, Fault::ChangedBeforeDelete)
            && path
                .to_string()
                .eq_ignore_ascii_case("Garmin/Mock/europe.img")
            && !self.triggered.swap(true, Ordering::SeqCst)
        {
            tokio::fs::write(&self.path, b"changed outside the transaction").await?;
        }
        self.inner.delete_size_checked(storage, path, size).await
    }

    async fn upload(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        source: &Path,
        size: u64,
        sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), DeviceIoError> {
        if !self.triggered.swap(true, Ordering::SeqCst) {
            match self.fault {
                Fault::Upload => {
                    return Err(DeviceIoError::Transport(
                        "injected disconnect during upload".to_owned(),
                    ));
                }
                Fault::UploadAcceptance => {
                    self.inner
                        .upload(storage, path, source, size, sha256, progress)
                        .await?;
                    return Err(DeviceIoError::Verification(path.to_string()));
                }
                Fault::ChangedBeforeDelete
                | Fault::DisconnectDuringBackup
                | Fault::Restore
                | Fault::ClearTransaction => {}
            }
        }
        self.inner
            .upload(storage, path, source, size, sha256, progress)
            .await
    }

    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        if matches!(self.fault, Fault::Restore) {
            return Err(DeviceIoError::Transport(
                "injected restore failure".to_owned(),
            ));
        }
        self.inner
            .restore(storage, path, size, backup, sha256)
            .await
    }
}

#[tokio::test]
async fn unsupported_authorization_stops_before_device_mutation() -> Result<()> {
    let fixture = Fixture::new(MockAuthorization::SignedStorageData).await?;
    let result = execute_update_plan(fixture.execution(Box::new(PhysicalTarget(Box::new(
        DirectoryDevice::new(fixture.device.clone()),
    )))))
    .await;

    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("signed storage data")
    );
    for old in ["europe-old.img", "trails-old.img", "global-old.img"] {
        assert_eq!(tokio::fs::read(fixture.old_path(old)).await?, OLD);
    }
    for new in ["europe.img", "europe-routing.img", "trails.img"] {
        assert!(!fixture.new_path(new).exists());
    }
    assert!(fixture.capture.root().join("error.txt").is_file());
    assert_capacity_evidence(&fixture, true);
    assert!(!fixture.capture.root().join("complete.json").exists());
    fixture.server.shutdown().await?;
    drop(fixture.root);
    Ok(())
}

#[tokio::test]
async fn changed_original_is_preserved_and_other_writes_roll_back() -> Result<()> {
    let fixture = Fixture::new(MockAuthorization::Supported).await?;
    tokio::fs::write(fixture.new_path("europe.img"), b"installed original").await?;
    let result = execute_update_plan(fixture.execution(Box::new(PhysicalTarget(Box::new(
        FaultDevice::new(&fixture.device, Fault::ChangedBeforeDelete),
    )))))
    .await;

    assert!(result.is_err());
    assert_eq!(
        tokio::fs::read(fixture.new_path("europe.img")).await?,
        b"changed outside the transaction"
    );
    for old in ["europe-old.img", "trails-old.img", "global-old.img"] {
        assert_eq!(tokio::fs::read(fixture.old_path(old)).await?, OLD);
    }
    for new in ["europe-routing.img", "trails.img"] {
        assert!(!fixture.new_path(new).exists());
    }
    assert!(fixture.capture.root().join("error.txt").is_file());
    assert_capacity_evidence(&fixture, true);
    assert!(!fixture.capture.root().join("complete.json").exists());
    fixture.server.shutdown().await?;
    drop(fixture.root);
    Ok(())
}

#[tokio::test]
async fn physical_failures_restore_device_state_and_retain_diagnostics() -> Result<()> {
    for fault in [Fault::Upload, Fault::UploadAcceptance] {
        let fixture = Fixture::new(MockAuthorization::Supported).await?;
        let result = execute_update_plan(fixture.execution(Box::new(PhysicalTarget(Box::new(
            FaultDevice::new(&fixture.device, fault),
        )))))
        .await;

        assert!(result.is_err());
        assert_pristine(&fixture).await?;
        assert_failure_capture(&fixture, true).await?;
        assert!(
            fixture
                .capture
                .root()
                .join("mounted-update/transaction/rolled-back.json")
                .is_file()
        );
        fixture.server.shutdown().await?;
        drop(fixture.root);
    }
    Ok(())
}

#[tokio::test]
async fn simulated_disconnect_preserves_source_and_records_the_failure() -> Result<()> {
    let fixture = Fixture::new(MockAuthorization::Supported).await?;
    let source = Box::new(FaultDevice::new(
        &fixture.device,
        Fault::DisconnectDuringBackup,
    ));
    let target = SimulatedTarget {
        source,
        fixture: Some(fixture.device.clone()),
        write_bytes_per_second: None,
    };
    let result = execute_update_plan(fixture.execution(Box::new(target))).await;

    assert!(result.is_err());
    assert_pristine(&fixture).await?;
    assert_failure_capture(&fixture, false).await?;
    assert!(
        fixture
            .capture
            .root()
            .join("simulation/README.md")
            .is_file()
    );
    assert!(
        !fixture
            .capture
            .root()
            .join("simulation/changes.json")
            .exists()
    );
    fixture.server.shutdown().await?;
    drop(fixture.root);
    Ok(())
}

#[tokio::test]
async fn cancelled_writes_clear_recovery_only_after_verified_rollback() -> Result<()> {
    for (simulated, write_bytes_per_second) in [
        (false, None),
        (true, None),
        (true, std::num::NonZeroU64::new(1_000_000)),
    ] {
        let fixture = Fixture::new(MockAuthorization::Supported).await?;
        let receipts = PendingRecoveryStore::new(fixture.root.path().join("receipts"));
        let progress = cancelling_commit_progress();
        let device = DirectoryDevice::new(fixture.device.clone());
        let target: Box<dyn garmin_services::maps::UpdateTarget> = if simulated {
            Box::new(SimulatedTarget {
                source: Box::new(device),
                fixture: None,
                write_bytes_per_second,
            })
        } else {
            Box::new(PhysicalTarget(Box::new(device)))
        };
        let mut execution = fixture.execution(target);
        execution.progress = progress.clone();
        let result = execute_registered_update(execution, Some(&receipts)).await;
        assert!(
            progress.is_cancelled(),
            "cancel after mutation, not during preparation"
        );
        assert!(
            result.is_err(),
            "cancellation must remain an unsuccessful operation"
        );
        assert_pristine(&fixture).await?;
        assert!(
            fixture
                .capture
                .root()
                .join("mounted-update/transaction/rolled-back.json")
                .is_file()
        );
        assert!(
            receipts
                .for_selected_device(&fixture.manifest.identity_digest(), None)?
                .is_none(),
            "verified rollback must clear its host notice"
        );
        let reopened = PendingRecoveryStore::new(fixture.root.path().join("receipts"));
        assert!(
            PendingState::inspect(
                &DirectoryDevice::new(fixture.device.clone()),
                &fixture.manifest.identity_digest(),
                &reopened,
            )
            .await?
            .is_none(),
            "refresh/restart must not ask for recovery again"
        );
        assert!(
            fixture.capture.root().join("error.txt").is_file(),
            "retain failure evidence"
        );
        fixture.server.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn failed_cancellation_rollback_retains_update_recovery() -> Result<()> {
    for fault in [Fault::Restore, Fault::ClearTransaction] {
        let fixture = Fixture::new(MockAuthorization::Supported).await?;
        tokio::fs::write(fixture.new_path("europe.img"), b"installed original").await?;
        let receipts = PendingRecoveryStore::new(fixture.root.path().join("receipts"));
        let progress = cancelling_commit_progress();
        let mut execution = fixture.execution(Box::new(PhysicalTarget(Box::new(
            FaultDevice::new(&fixture.device, fault),
        ))));
        execution.progress = progress.clone();
        let error = execute_registered_update(execution, Some(&receipts))
            .await
            .err()
            .expect("cancelled update");
        assert!(progress.is_cancelled());
        assert!(
            matches!(
                error.downcast_ref::<garmin_update::MountedInstallError>(),
                Some(garmin_update::MountedInstallError::Rollback { .. })
            ),
            "{error:#}"
        );
        let pending = PendingState::inspect(
            &DirectoryDevice::new(fixture.device.clone()),
            &fixture.manifest.identity_digest(),
            &receipts,
        )
        .await?
        .expect("failed rollback requires recovery");
        assert!(pending.receipt.is_some());
        if matches!(fault, Fault::Restore) {
            assert!(pending.transaction.is_some());
        }
        if matches!(fault, Fault::ClearTransaction) {
            assert!(
                fixture
                    .capture
                    .root()
                    .join("mounted-update/transaction/rolled-back.json")
                    .is_file(),
                "a rollback journal alone is not proof that device transaction cleanup succeeded"
            );
            assert_eq!(
                tokio::fs::read(fixture.new_path("europe.img")).await?,
                b"installed original"
            );
        }
        fixture.server.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn cancelled_removal_clears_notice_only_when_restoration_is_proven() -> Result<()> {
    for fault in [None, Some(Fault::Restore), Some(Fault::ClearTransaction)] {
        let fixture = Fixture::new(MockAuthorization::Supported).await?;
        let receipts = PendingRecoveryStore::new(fixture.root.path().join("receipts"));
        let mut catalog = fixture
            .client
            .check_maps(
                fixture.manifest.raw_xml(),
                fixture.manifest.capabilities().installed_map_files(),
            )
            .await?;
        // Both old files exist in this fixture; advertise both as installed for multi-file removal.
        catalog.bundled_maps[0].installation_state = catalog.maps[0].installation_state;
        let plan = garmin_update::RemovalPlan::from_response_selection(
            &catalog,
            fixture.manifest.identity_digest(),
            [0, 1],
        )?;
        let device: Box<dyn DeviceWrite> = if let Some(fault) = fault {
            Box::new(FaultDevice::new(&fixture.device, fault))
        } else {
            Box::new(DirectoryDevice::new(fixture.device.clone()))
        };
        let plan = plan.bind_inventory(&device.inventory(&plan.paths_to_inventory()).await?)?;
        assert!(plan.files_to_remove.len() > 1, "cancel between removals");
        let progress = cancelling_commit_progress();
        let error = garmin_services::maps::execute_removal(
            &plan,
            &fixture.capture,
            &progress,
            device.as_ref(),
            &receipts,
        )
        .await
        .expect_err("cancelled removal");
        assert!(progress.is_cancelled());
        let pending = PendingState::inspect(
            &DirectoryDevice::new(fixture.device.clone()),
            &fixture.manifest.identity_digest(),
            &receipts,
        )
        .await?;
        if fault.is_some() {
            assert!(
                matches!(
                    error.downcast_ref::<garmin_update::RemovalExecutionError>(),
                    Some(garmin_update::RemovalExecutionError::Rollback { .. })
                ),
                "{error:#}"
            );
            let pending = pending.expect("failed rollback requires recovery");
            assert!(pending.receipt.is_some());
            if matches!(fault, Some(Fault::Restore)) {
                assert!(pending.transaction.is_some());
            }
        } else {
            assert!(
                matches!(
                    error.downcast_ref::<garmin_update::RemovalExecutionError>(),
                    Some(garmin_update::RemovalExecutionError::RolledBack(_))
                ),
                "{error:#}"
            );
            assert!(pending.is_none());
            assert_pristine(&fixture).await?;
            assert!(
                fixture
                    .capture
                    .root()
                    .join("removal-transaction/rolled-back.json")
                    .is_file()
            );
        }
        fixture.server.shutdown().await?;
    }
    Ok(())
}

fn cancelling_commit_progress() -> ProgressReporter {
    let progress = ProgressReporter::default();
    let cancellation = progress.cancellation_token();
    progress.observe(move |event| {
        if event.stage == OperationStage::Commit
            && event.state == ProgressState::Advanced
            && event.completed > 0
            && event.path.is_some()
        {
            cancellation.cancel();
        }
    })
}

#[tokio::test]
async fn simulated_cancellation_preserves_source_and_reports_virtual_state() -> Result<()> {
    let fixture = Fixture::new(MockAuthorization::Supported).await?;
    let progress = ProgressReporter::default();
    let cancellation = progress.cancellation_token();
    let progress = progress.observe(move |event| {
        if event.stage == OperationStage::Commit && event.state == ProgressState::Started {
            cancellation.cancel();
        }
    });
    let target = SimulatedTarget {
        source: Box::new(DirectoryDevice::new(fixture.device.clone())),
        fixture: Some(fixture.device.clone()),
        write_bytes_per_second: None,
    };
    let mut execution = fixture.execution(Box::new(target));
    execution.progress = progress;
    let result = execute_update_plan(execution).await;

    assert!(result.is_err());
    assert_pristine(&fixture).await?;
    assert_failure_capture(&fixture, true).await?;
    assert!(
        fixture
            .capture
            .root()
            .join("simulation/changes.json")
            .is_file()
    );
    fixture.server.shutdown().await?;
    drop(fixture.root);
    Ok(())
}

#[tokio::test]
async fn recovery_rejects_a_different_device_without_changes() -> Result<()> {
    let fixture = Fixture::new(MockAuthorization::Supported).await?;
    execute_update_plan(fixture.execution(Box::new(PhysicalTarget(Box::new(
        DirectoryDevice::new(fixture.device.clone()),
    )))))
    .await?;
    let before = updated_files(&fixture).await?;

    let result = recover_update(RecoveryExecution {
        transaction: fixture.capture.root().to_owned(),
        device_digest: "different-device".to_owned(),
        device: Box::new(DirectoryDevice::new(fixture.device.clone())),
        progress: ProgressReporter::default(),
    })
    .await;

    assert!(result.unwrap_err().to_string().contains("different device"));
    assert_eq!(updated_files(&fixture).await?, before);
    fixture.server.shutdown().await?;
    drop(fixture.root);
    Ok(())
}

async fn updated_files(fixture: &Fixture) -> Result<Vec<Vec<u8>>> {
    let mut files = Vec::new();
    for name in ["europe.img", "europe-routing.img", "trails.img"] {
        files.push(tokio::fs::read(fixture.new_path(name)).await?);
    }
    Ok(files)
}

async fn assert_pristine(fixture: &Fixture) -> Result<()> {
    for old in ["europe-old.img", "trails-old.img", "global-old.img"] {
        assert_eq!(tokio::fs::read(fixture.old_path(old)).await?, OLD);
    }
    for new in ["europe.img", "europe-routing.img", "trails.img"] {
        assert!(!fixture.new_path(new).exists());
    }
    Ok(())
}

async fn assert_failure_capture(fixture: &Fixture, has_final_state: bool) -> Result<()> {
    assert!(
        !tokio::fs::read(fixture.capture.root().join("error.txt"))
            .await?
            .is_empty()
    );
    assert_capacity_evidence(fixture, has_final_state);
    assert!(!fixture.capture.root().join("complete.json").exists());
    Ok(())
}

fn assert_capacity_evidence(fixture: &Fixture, has_final_state: bool) {
    assert!(
        fixture
            .capture
            .root()
            .join("device-state-before.json")
            .is_file()
    );
    assert_eq!(
        fixture
            .capture
            .root()
            .join("device-state-after.json")
            .is_file(),
        has_final_state
    );
}
