use std::{
    path::Path,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use async_trait::async_trait;
use garmin_capture::SessionCapture;
use garmin_device::{
    BackupDestination, DeviceInventory, DevicePathState, DeviceStateSnapshot,
    MountedMtpBackupProgress, MountedMtpUploadProgress, SafeRelativePath, TransportKind,
    parse_manifest,
    storage::{DeviceIoError, DeviceRead, DeviceWrite, DirectoryDevice},
};
use garmin_map_service::{ClientIdentity, OmtClient};
use garmin_progress::{OperationStage, ProgressReporter, ProgressState};
use garmin_services::maps::{
    GarminDownloadAuthorizer, PhysicalTarget, RecoveryExecution, SimulatedTarget, UpdateExecution,
    execute_update_plan, recover_update,
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
    Readback,
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
    ) -> Result<(DevicePathState, Option<u64>), DeviceIoError> {
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
}

#[async_trait]
impl DeviceWrite for FaultDevice {
    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
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
                Fault::Readback => {
                    self.inner
                        .upload(storage, path, source, size, sha256, progress)
                        .await?;
                    return Err(DeviceIoError::Verification(path.to_string()));
                }
                Fault::ChangedBeforeDelete | Fault::DisconnectDuringBackup => {}
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
    for fault in [Fault::Upload, Fault::Readback] {
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
