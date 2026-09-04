//! Device operations through the virtual MTP responder.

mod support;

use std::{error::Error, fs, io, time::Duration};

use bytes::Bytes;
use futures_lite::{future::block_on, stream};
use garmin_device::capabilities::parse;
use garmin_device::{
    DevicePathState, RawMtpLink, RawMtpSession, SafeRelativePath, TransportKind, UploadCompletion,
    inventory_mtp,
    storage::{DeviceLink, DeviceProbeRequest},
};
use garmin_progress::{OperationStage, ProgressReporter, ProgressState};
use mtp_rs::{
    MtpDevice, NewObjectInfo, Storage, VirtualDeviceConfig, VirtualStorageConfig,
    register_virtual_device, unregister_virtual_device,
};
use sha2::{Digest as _, Sha256};
use tempfile::{TempDir, tempdir};

const GARMIN_DEVICE_V2_NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";
const ORIGINAL: &[u8] = b"unchanged map";
const PAYLOAD: &[u8] = b"synthetic map payload";

#[test]
fn reads_and_parses_manifest_through_virtual_mtp() -> Result<(), Box<dyn Error>> {
    block_on(async {
        let manifest_xml = support::manifest(
            GARMIN_DEVICE_V2_NAMESPACE,
            &[("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit")],
        )?;
        let backing = tempdir()?;
        let garmin = backing.path().join("GARMIN");
        fs::create_dir(&garmin)?;
        fs::write(garmin.join("GarminDevice.xml"), manifest_xml)?;

        let device = MtpDevice::builder()
            .open_virtual(VirtualDeviceConfig {
                serial: "garmin-toolkit-synthetic-device".to_owned(),
                storages: vec![VirtualStorageConfig {
                    description: "Internal Storage".to_owned(),
                    capacity: 1_048_576,
                    backing_dir: backing.path().to_path_buf(),
                    read_only: true,
                }],
                event_poll_interval: Duration::ZERO,
                watch_backing_dirs: false,
                ..VirtualDeviceConfig::default()
            })
            .await?;

        let storage = device
            .storages()
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("virtual device exposed no storage"))?;
        let manifest = storage
            .list_objects_recursive(None)
            .await?
            .into_iter()
            .find(|object| object.filename == "GarminDevice.xml")
            .ok_or_else(|| io::Error::other("virtual device exposed no manifest"))?;
        let bytes = storage.download_to_vec(manifest.handle).await?;
        let xml = String::from_utf8(bytes)?;
        let parsed = parse(&xml)?;

        assert_eq!(parsed.id().as_u32(), 123_456);
        assert_eq!(parsed.model().description(), "Synthetic Garmin");
        assert_eq!(parsed.capabilities().len(), 1);
        Ok(())
    })
}

#[test]
fn uploaded_and_deleted_objects_reach_production_inventory() -> Result<(), Box<dyn Error>> {
    block_on(async {
        let fixture = RegisteredDevice::new(false)?;
        let (device, storage) = fixture.open().await?;
        let garmin = storage.create_folder(None, "Garmin").await?;
        let size = u64::try_from(PAYLOAD.len())?;
        storage
            .upload(
                Some(garmin),
                NewObjectInfo::file("map.img", size),
                stream::once(Ok(Bytes::from_static(PAYLOAD))),
            )
            .await?;
        device.close().await?;

        let path = SafeRelativePath::parse("Garmin/map.img")?;
        let inventory = inventory_mtp(fixture.location_id, std::slice::from_ref(&path)).await?;
        assert_eq!(inventory.transport, TransportKind::Mtp);
        assert_eq!(inventory.paths.len(), 1);
        assert_eq!(inventory.paths[0].state, DevicePathState::RegularFile);
        assert_eq!(inventory.paths[0].size, Some(size));

        let (device, storage) = fixture.open().await?;
        let uploaded = storage
            .list_objects_recursive(None)
            .await?
            .into_iter()
            .find(|object| object.filename == "map.img")
            .ok_or_else(|| io::Error::other("reopened device lost the upload"))?;
        assert_eq!(storage.download_to_vec(uploaded.handle).await?, PAYLOAD);
        storage.delete(uploaded.handle).await?;
        device.close().await?;

        let inventory = inventory_mtp(fixture.location_id, &[path]).await?;
        assert_eq!(inventory.paths.len(), 1);
        assert_eq!(inventory.paths[0].state, DevicePathState::Missing);
        assert_eq!(inventory.paths[0].size, None);
        assert_eq!(
            fs::read(fixture.backing.path().join("unchanged.img"))?,
            ORIGINAL
        );
        Ok(())
    })
}

#[test]
fn read_only_virtual_storage_rejects_mutation() -> Result<(), Box<dyn Error>> {
    block_on(async {
        let fixture = RegisteredDevice::new(true)?;
        let (device, storage) = fixture.open().await?;
        let original = storage
            .list_objects(None)
            .await?
            .into_iter()
            .find(|object| object.filename == "unchanged.img")
            .ok_or_else(|| io::Error::other("virtual device lost the original"))?;
        let upload = storage
            .upload(
                None,
                NewObjectInfo::file("map.img", u64::try_from(PAYLOAD.len())?),
                stream::once(Ok(Bytes::from_static(PAYLOAD))),
            )
            .await
            .expect_err("read-only storage must reject uploads");
        assert!(matches!(upload.source, mtp_rs::Error::AccessDenied));
        assert!(upload.partial.is_none());
        assert!(matches!(
            storage.delete(original.handle).await,
            Err(mtp_rs::Error::AccessDenied)
        ));
        assert_eq!(storage.download_to_vec(original.handle).await?, ORIGINAL);
        device.close().await?;

        let path = SafeRelativePath::parse("map.img")?;
        let inventory = inventory_mtp(fixture.location_id, &[path]).await?;
        assert_eq!(inventory.paths.len(), 1);
        assert_eq!(inventory.paths[0].state, DevicePathState::Missing);
        assert_eq!(
            fs::read(fixture.backing.path().join("unchanged.img"))?,
            ORIGINAL
        );
        Ok(())
    })
}

#[tokio::test]
async fn link_probe_reads_back_and_verifies_the_virtual_mtp_object() -> Result<(), Box<dyn Error>> {
    let fixture = RegisteredDevice::new(false)?;
    fs::create_dir(fixture.backing.path().join("Garmin"))?;
    let source_dir = tempdir()?;
    let source = source_dir.path().join("probe.bin");
    fs::write(&source, vec![0x5a; 512 * 1024])?;
    let (progress, receiver) = ProgressReporter::channel();
    let recovery = tempdir()?;

    let report = RawMtpLink::new(fixture.location_id)
        .probe(
            &DeviceProbeRequest {
                source,
                recovery_root: recovery.path().to_owned(),
                device_identity: "virtual-probe-device".to_owned(),
            },
            &progress,
        )
        .await?;
    let events = receiver.try_iter().collect::<Vec<_>>();

    assert!(report.content_verified);
    assert!(report.read_back_seconds > 0.0);
    assert_eq!(report.upload_completion, UploadCompletion::Acknowledged);
    assert!(report.temporary_file_removed);
    assert!(recovery.path().read_dir()?.next().is_none());
    let upload_complete = events
        .iter()
        .position(|event| {
            event.stage == OperationStage::Upload && event.state == ProgressState::Completed
        })
        .ok_or_else(|| io::Error::other("payload completion was not reported"))?;
    let finalize_started = events
        .iter()
        .position(|event| {
            event.stage == OperationStage::DeviceFinalize && event.state == ProgressState::Started
        })
        .ok_or_else(|| io::Error::other("device finalization was not reported"))?;
    assert!(upload_complete < finalize_started);
    assert!(events.iter().any(|event| {
        event.stage == OperationStage::DeviceVerify
            && event.state == ProgressState::Advanced
            && event.completed == report.bytes
    }));
    assert!(
        fs::read_dir(fixture.backing.path().join("Garmin"))?
            .next()
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn one_raw_session_covers_manifest_capacity_and_probe() -> Result<(), Box<dyn Error>> {
    let fixture = RegisteredDevice::new(false)?;
    let garmin = fixture.backing.path().join("Garmin");
    fs::create_dir(&garmin)?;
    fs::write(
        garmin.join("GarminDevice.xml"),
        support::manifest(
            GARMIN_DEVICE_V2_NAMESPACE,
            &[("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit")],
        )?,
    )?;
    let source_dir = tempdir()?;
    let source = source_dir.path().join("probe.bin");
    fs::write(&source, vec![0x5a; 128 * 1024])?;
    let recovery = tempdir()?;
    let progress = ProgressReporter::default();

    let (session, manifest) = RawMtpSession::open(fixture.location_id).await?;
    assert_eq!(manifest.summary.model, "Synthetic Garmin");
    assert_eq!(session.capacity().await?.state.storages.len(), 1);
    let report = session
        .probe(
            &DeviceProbeRequest {
                source,
                recovery_root: recovery.path().to_owned(),
                device_identity: manifest.identity_digest(),
            },
            &progress,
        )
        .await?;

    assert!(report.content_verified);
    assert!(report.temporary_file_removed);
    assert!(recovery.path().read_dir()?.next().is_none());
    assert_eq!(
        fs::read(garmin.join("GarminDevice.xml"))?,
        manifest.raw_xml().as_bytes(),
    );
    Ok(())
}

#[tokio::test]
async fn a_persisted_receipt_recovers_an_object_before_the_next_probe() -> Result<(), Box<dyn Error>>
{
    let fixture = RegisteredDevice::new(false)?;
    let (device, storage) = fixture.open().await?;
    let garmin = storage.create_folder(None, "Garmin").await?;
    let recovery = tempdir()?;
    let object_name = ".garmin-cli-probe-interrupted.bin";
    let bytes = u64::try_from(PAYLOAD.len())?;
    storage
        .upload(
            Some(garmin),
            NewObjectInfo::file(object_name, bytes),
            stream::once(Ok(Bytes::from_static(PAYLOAD))),
        )
        .await?;
    let receipt = serde_json::json!({
        "version": 1,
        "device_identity": "virtual-probe-device",
        "storage_id": storage.id().0,
        "object_name": object_name,
        "bytes": bytes,
        "sha256": hex::encode(Sha256::digest(PAYLOAD)),
    });
    fs::write(
        recovery.path().join("interrupted.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    device.close().await?;

    let source_dir = tempdir()?;
    let source = source_dir.path().join("probe.bin");
    fs::write(&source, PAYLOAD)?;
    let (progress, receiver) = ProgressReporter::channel();
    let report = RawMtpLink::new(fixture.location_id)
        .probe(
            &DeviceProbeRequest {
                source,
                recovery_root: recovery.path().to_owned(),
                device_identity: "virtual-probe-device".to_owned(),
            },
            &progress,
        )
        .await?;
    let events = receiver.try_iter().collect::<Vec<_>>();

    assert!(report.content_verified);
    assert!(events.iter().any(|event| {
        event.stage == OperationStage::DeviceFinalize
            && event.state == ProgressState::Completed
            && event.label.contains("earlier disposable MTP object")
    }));
    assert!(recovery.path().read_dir()?.next().is_none());
    assert!(
        fs::read_dir(fixture.backing.path().join("Garmin"))?
            .next()
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn a_cancelled_upload_removes_its_partial_object_and_receipt() -> Result<(), Box<dyn Error>> {
    let fixture = RegisteredDevice::new(false)?;
    fs::create_dir(fixture.backing.path().join("Garmin"))?;
    let source_dir = tempdir()?;
    let source = source_dir.path().join("probe.bin");
    fs::write(&source, vec![0x5a; 512 * 1024])?;
    let recovery = tempdir()?;
    let progress = ProgressReporter::default();
    progress.cancellation_token().cancel();

    let result = RawMtpLink::new(fixture.location_id)
        .probe(
            &DeviceProbeRequest {
                source,
                recovery_root: recovery.path().to_owned(),
                device_identity: "virtual-probe-device".to_owned(),
            },
            &progress,
        )
        .await;

    assert!(result.is_err());
    assert!(recovery.path().read_dir()?.next().is_none());
    assert!(
        fs::read_dir(fixture.backing.path().join("Garmin"))?
            .next()
            .is_none()
    );
    Ok(())
}

struct RegisteredDevice {
    backing: TempDir,
    location_id: u64,
}

impl RegisteredDevice {
    fn new(read_only: bool) -> Result<Self, Box<dyn Error>> {
        let backing = tempdir()?;
        fs::write(backing.path().join("unchanged.img"), ORIGINAL)?;
        let info = register_virtual_device(&VirtualDeviceConfig {
            serial: format!("garmin-toolkit-test-{}", uuid::Uuid::new_v4()),
            storages: vec![VirtualStorageConfig {
                description: "Synthetic storage".to_owned(),
                capacity: 1_048_576,
                backing_dir: backing.path().to_path_buf(),
                read_only,
            }],
            event_poll_interval: Duration::ZERO,
            watch_backing_dirs: false,
            ..VirtualDeviceConfig::default()
        });
        Ok(Self {
            backing,
            location_id: info.location_id,
        })
    }

    async fn open(&self) -> Result<(MtpDevice, Storage), Box<dyn Error>> {
        let device = MtpDevice::builder()
            .open_by_location(self.location_id)
            .await?;
        let storage = device
            .storages()
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("virtual device exposed no storage"))?;
        Ok((device, storage))
    }
}

impl Drop for RegisteredDevice {
    fn drop(&mut self) {
        unregister_virtual_device(self.location_id);
    }
}
