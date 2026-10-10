use super::*;
use garmin_device::{
    DeviceStateSnapshot, DeviceStorageState, SafeRelativePath, StorageCapacity,
    storage::{DeviceIoError, DirectoryDevice},
};

struct Volumes {
    internal: DirectoryDevice,
    card: DirectoryDevice,
}

#[async_trait::async_trait]
impl DeviceRead for Volumes {
    fn execution_target(&self) -> Option<&str> {
        None
    }
    async fn state(&self) -> Result<DeviceStateSnapshot, DeviceIoError> {
        Ok(DeviceStateSnapshot {
            storages: vec![
                DeviceStorageState {
                    id: "internal".to_owned(),
                    label: "Internal".to_owned(),
                    capacity: StorageCapacity::new(100, 20),
                    writable: Some(true),
                },
                DeviceStorageState {
                    id: "card".to_owned(),
                    label: "Card".to_owned(),
                    capacity: StorageCapacity::unavailable("capacity not reported"),
                    writable: Some(false),
                },
            ],
        })
    }
    async fn inventory(
        &self,
        _: &[SafeRelativePath],
    ) -> Result<garmin_device::DeviceInventory, DeviceIoError> {
        panic!("inspection must not crawl files")
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        panic!("inspection must retain all volumes")
    }
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<garmin_device::DevicePathStatus, DeviceIoError> {
        match storage {
            "internal" => self.internal.inspect(storage, path).await,
            "card" => self.card.inspect(storage, path).await,
            _ => panic!("unknown volume"),
        }
    }
    async fn list_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<garmin_device::storage::DeviceDirectoryEntry>, DeviceIoError> {
        match storage {
            "internal" => self.internal.list_directory(storage, path).await,
            "card" => self.card.list_directory(storage, path).await,
            _ => panic!("unknown volume"),
        }
    }
    async fn backup(
        &self,
        _: &str,
        _: &SafeRelativePath,
        _: u64,
        _: garmin_device::BackupDestination,
        _: garmin_device::MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        panic!("unexpected backup")
    }
    async fn verify(
        &self,
        _: &str,
        _: &SafeRelativePath,
        _: u64,
        _: &str,
    ) -> Result<(), DeviceIoError> {
        panic!("unexpected verification")
    }
    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        match storage {
            "internal" => self.internal.read_bounded_file(storage, path, limit).await,
            "card" => self.card.read_bounded_file(storage, path, limit).await,
            _ => panic!("unknown volume"),
        }
    }
}

#[tokio::test]
async fn bad_manifest_and_one_bad_volume_preserve_other_results() {
    let directory = tempfile::tempdir().unwrap();
    let card = tempfile::tempdir().unwrap();
    std::fs::create_dir(card.path().join("GARMIN-TOOLKIT")).unwrap();
    std::fs::write(card.path().join("GARMIN-TOOLKIT/identity.toml"), b"bad").unwrap();
    let device = Volumes {
        internal: DirectoryDevice::with_storage(
            directory.path().to_owned(),
            "internal",
            "Internal",
        ),
        card: DirectoryDevice::with_storage(card.path().to_owned(), "card", "Card"),
    };
    let metadata = inspect_attachment(
        &device,
        "Attached device",
        Err("bad Garmin manifest".to_owned()),
    )
    .await;
    assert_eq!(metadata.id, None);
    assert_eq!(metadata.storage.storages.len(), 2);
    let report = metadata.report.unwrap();
    assert!(report.has_errors());
    assert_eq!(report.toolkit[0].namespace, InspectionSection::Missing);
    assert_eq!(report.toolkit[0].identity, InspectionSection::Missing);
    assert!(matches!(
        report.toolkit[1].identity,
        InspectionSection::Unavailable(_)
    ));
    assert_eq!(report.toolkit[1].transaction, InspectionSection::Missing);
}

#[tokio::test]
async fn attachment_and_cli_orchestration_produce_the_same_report() {
    let directory = tempfile::tempdir().unwrap();
    let fixture =
        garmin_fixtures::device::Device::recreate(directory.path().join("device")).unwrap();
    let manifest = garmin_fixtures::device::manifest();
    let device = fixture.transport();
    let canonical = inspect(
        &device,
        InspectionSection::Available(manifest_inspection(&manifest)),
    )
    .await;
    let attachment =
        inspect_attachment(&device, "fixture", Ok(garmin_fixtures::device::metadata())).await;
    let report = attachment.report.unwrap();
    assert_eq!(canonical.manifest, report.manifest);
    assert_eq!(canonical.toolkit, report.toolkit);
    assert!(!report.has_errors());
    assert_eq!(report.toolkit.len(), 1);
}
