use crate::manifest::{MAX_MANIFEST_BYTES, ManifestError, parse_manifest};
use crate::storage::raw_mtp::{RawMtpUploadOutcome, RawMtpUploadProgress, upload_mtp_file};
use crate::storage::{DeviceLink, DeviceLinkCapacity, DeviceLinkError, DeviceProbeRequest};
use crate::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState, SafeRelativePath,
    TransportKind,
};
use crate::{DeviceProbeReport, UploadCompletion};
use garmin_progress::{OperationStage, ProgressReporter};
use mtp_rs::ptp::{ObjectHandle as PtpObjectHandle, OperationCode, PtpDevice, ResponseCode};
use mtp_rs::{ByteRange, MtpDevice, MtpDeviceBuilder, ObjectHandle, Storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

const PROBE_RECEIPT_VERSION: u8 = 1;
const GARMIN_MTP_METADATA_TIMEOUT: Duration = Duration::from_secs(10);
const GARMIN_MTP_TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const GARMIN_MTP_RESET_QUIET_PERIOD: Duration = Duration::from_secs(5);
const FENIX_8_SOLAR_47_MTP_PRODUCT_ID: u16 = 0x51b4;
const USB_TOPOLOGY_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const USB_TOPOLOGY_HASH_PRIME: u64 = 0x0100_0000_01b3;
const USB_TOPOLOGY_FIELD_SEPARATOR: u8 = 0xff;
const MTP_RECOVERY_POLICY: RetryPolicy = RetryPolicy {
    attempt_limit: 3,
    initial_delay: Duration::from_secs(2),
    delay_multiplier: 2,
};

#[derive(Clone, Copy)]
struct RetryPolicy {
    attempt_limit: usize,
    initial_delay: Duration,
    delay_multiplier: u32,
}

impl RetryPolicy {
    fn delays(self) -> impl Iterator<Item = Duration> {
        let mut next = self.initial_delay;
        (0..self.attempt_limit).map(move |_| {
            let delay = next;
            next = next.saturating_mul(self.delay_multiplier);
            delay
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProbeReceipt {
    version: u8,
    device_identity: String,
    storage_id: u64,
    object_name: String,
    bytes: u64,
    sha256: String,
}

struct ProbeLease {
    path: PathBuf,
    receipt: ProbeReceipt,
}

impl ProbeLease {
    async fn create(
        root: &Path,
        device_identity: &str,
        storage_id: u64,
        bytes: u64,
        sha256: String,
    ) -> Result<Self, MtpError> {
        tokio::fs::create_dir_all(root).await?;
        let id = Uuid::new_v4();
        let receipt = ProbeReceipt {
            version: PROBE_RECEIPT_VERSION,
            device_identity: device_identity.to_owned(),
            storage_id,
            object_name: format!(".garmin-cli-probe-{id}.bin"),
            bytes,
            sha256,
        };
        let path = root.join(format!("{id}.json"));
        let temporary = root.join(format!(".{id}.tmp"));
        let contents = serde_json::to_vec_pretty(&receipt).map_err(std::io::Error::other)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &contents).await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, &path).await?;
        Ok(Self { path, receipt })
    }

    async fn remove(self) -> Result<(), MtpError> {
        tokio::fs::remove_file(self.path).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RawMtpLink {
    location_id: u64,
}

/// One caller-owned raw-MTP session retained across inspection and transfer.
///
/// Garmin responders are known to behave badly when a command-line workflow
/// repeatedly opens and closes sessions. This type makes the session lifetime
/// explicit for workflows that need more than one operation.
pub struct RawMtpSession {
    location_id: u64,
    device: MtpDevice,
}

impl RawMtpSession {
    /// Open the selected Garmin once and read its manifest without closing the session.
    ///
    /// # Errors
    /// The device cannot be opened or its canonical manifest cannot be read.
    pub async fn open(location_id: u64) -> Result<(Self, DeviceManifest), MtpError> {
        let device = open_raw_mtp_with_timeout(location_id, GARMIN_MTP_METADATA_TIMEOUT).await?;
        let manifest = match read_manifest_from_open_device(&device, location_id).await {
            Ok(manifest) => manifest,
            Err(error) => return finish_device(device, Err(error)).await,
        };
        Ok((
            Self {
                location_id,
                device,
            },
            manifest,
        ))
    }

    /// Read storage capacity through the already-open session.
    ///
    /// # Errors
    /// Storage enumeration or selection failed.
    pub async fn capacity(&self) -> Result<DeviceLinkCapacity, MtpError> {
        let storages = self.device.storages().await?;
        let state = mtp_state(&storages);
        let storage_id = select_probe_storage(storages, 0).await?.0.id().0;
        Ok(DeviceLinkCapacity {
            state,
            storage_id: format!("{storage_id:016x}"),
        })
    }

    /// Run and clean up a link probe on the already-open session.
    ///
    /// The session is consumed because the probe owns its complete recovery
    /// and shutdown lifecycle.
    ///
    /// # Errors
    /// Probe preparation, transfer, verification, reconciliation, or cleanup failed.
    pub async fn probe(
        self,
        request: &DeviceProbeRequest,
        progress: &ProgressReporter,
    ) -> Result<DeviceProbeReport, MtpError> {
        probe_mtp_file_with_device(
            self.location_id,
            self.device,
            &request.source,
            &request.recovery_root,
            &request.device_identity,
            progress,
        )
        .await
    }
}

impl RawMtpLink {
    #[must_use]
    pub const fn new(location_id: u64) -> Self {
        Self { location_id }
    }
}

#[async_trait::async_trait]
impl DeviceLink for RawMtpLink {
    async fn capacity(&self) -> Result<Option<DeviceLinkCapacity>, DeviceLinkError> {
        let device = open_raw_mtp(self.location_id)
            .await
            .map_err(MtpError::from)?;
        let capacity = async {
            let storages = device.storages().await.map_err(MtpError::from)?;
            let state = mtp_state(&storages);
            let storage_id = select_probe_storage(storages, 0).await?.0.id().0;
            Ok(DeviceLinkCapacity {
                state,
                storage_id: format!("{storage_id:016x}"),
            })
        }
        .await;
        Ok(Some(finish_device(device, capacity).await?))
    }

    async fn probe(
        &self,
        request: &DeviceProbeRequest,
        progress: &ProgressReporter,
    ) -> Result<DeviceProbeReport, DeviceLinkError> {
        Ok(probe_mtp_file(
            self.location_id,
            &request.source,
            &request.recovery_root,
            &request.device_identity,
            progress,
        )
        .await?)
    }
}

pub(crate) async fn open_raw_mtp(location_id: u64) -> Result<MtpDevice, mtp_rs::Error> {
    open_raw_mtp_with_timeout(location_id, GARMIN_MTP_TRANSFER_TIMEOUT).await
}

async fn open_raw_mtp_with_timeout(
    location_id: u64,
    timeout: Duration,
) -> Result<MtpDevice, mtp_rs::Error> {
    MtpDeviceBuilder::new()
        .timeout(timeout)
        .known_devices(KNOWN_GARMIN_MTP)
        .open_by_location(location_id)
        .await
}

pub const GARMIN_USB_VENDOR_ID: u16 = 0x091e;
/// Garmin USB identities confirmed by the retained device research and update implementation.
pub const KNOWN_GARMIN_MTP: &[(u16, u16)] = &[
    (GARMIN_USB_VENDOR_ID, 0x5158),
    (GARMIN_USB_VENDOR_ID, 0x51b4),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MtpCandidate {
    pub location_id: u64,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: Option<String>,
    pub speed: Option<String>,
}

#[must_use]
pub fn discover_mtp_candidates() -> Vec<MtpCandidate> {
    MtpDevice::list_devices_with_known(KNOWN_GARMIN_MTP)
        .unwrap_or_default()
        .into_iter()
        .filter(|device| device.vendor_id == GARMIN_USB_VENDOR_ID)
        .map(|device| MtpCandidate {
            location_id: device.location_id,
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            product: device.product,
            speed: device.speed.map(|speed| format!("{speed:?}")),
        })
        .collect()
}

/// Whether retained hardware evidence has disproved automatic USB-reset recovery.
///
/// This lookup is deliberately location-bound so a result for one attached
/// Garmin model cannot suppress recovery for another.
#[must_use]
pub fn mtp_usb_reset_known_ineffective(location_id: u64) -> bool {
    discover_mtp_candidates().into_iter().any(|candidate| {
        candidate.location_id == location_id
            && usb_reset_known_ineffective(candidate.vendor_id, candidate.product_id)
    })
}

const fn usb_reset_known_ineffective(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == GARMIN_USB_VENDOR_ID && product_id == FENIX_8_SOLAR_47_MTP_PRODUCT_ID
}

/// Open one Garmin MTP device and retrieve `Garmin/GarminDevice.xml`.
///
/// Tries Garmin's `0x9001` shortcut, then each storage root and `Garmin` child.
/// # Errors
/// USB, session, listing, download, UTF-8, or manifest failure.
pub async fn open_mtp(location_id: u64) -> Result<DeviceManifest, MtpError> {
    match read_manifest_with_garmin_opcode(location_id).await {
        Ok(xml) => {
            return parse_manifest(&xml, TransportKind::Mtp, format!("mtp:{location_id:#x}"))
                .map_err(Into::into);
        }
        Err(MtpFastPathError::Connection(error)) => {
            return Err(MtpError::Protocol(error.into()));
        }
        Err(_) => {}
    }

    let device = open_raw_mtp_with_timeout(location_id, GARMIN_MTP_METADATA_TIMEOUT).await?;
    let manifest = read_manifest_from_open_device(&device, location_id).await;
    finish_device(device, manifest).await
}

async fn read_manifest_from_open_device(
    device: &MtpDevice,
    location_id: u64,
) -> Result<DeviceManifest, MtpError> {
    let mut manifest_bytes = None;
    for storage in device.storages().await? {
        let roots = storage.list_objects(None).await?;
        let garmin = roots
            .iter()
            .find(|item| item.is_folder() && item.filename.eq_ignore_ascii_case("Garmin"));
        let Some(garmin) = garmin else {
            continue;
        };
        let children = storage.list_objects(Some(garmin.handle)).await?;
        if let Some(manifest) = children
            .iter()
            .find(|item| item.is_file() && item.filename.eq_ignore_ascii_case("GarminDevice.xml"))
        {
            manifest_bytes = Some(storage.download_to_vec(manifest.handle).await?);
            break;
        }
    }

    let Some(bytes) = manifest_bytes else {
        return Err(MtpError::MissingManifest);
    };
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge.into());
    }
    let xml = String::from_utf8(bytes)?;
    parse_manifest(&xml, TransportKind::Mtp, format!("mtp:{location_id:#x}")).map_err(Into::into)
}

/// Reset an unresponsive Garmin USB device and leave it quiet before reopening.
///
/// Call this only after a bounded open attempt has timed out.
/// The target is checked against the known Garmin USB inventory before a full
/// USB port reset is sent, so an arbitrary MTP device cannot be reset through
/// this API. This deliberately matches libmtp's failed-`OpenSession` recovery;
/// Garmin's vendor-class MTP interface rejects the USB Still Image Class
/// `DEVICE_RESET` request.
///
/// # Errors
/// The location is not a currently attached Garmin MTP candidate, or USB reset failed.
pub async fn reset_mtp_transport(location_id: u64) -> Result<(), MtpError> {
    let devices = nusb::list_devices()
        .await
        .map_err(MtpError::UsbEnumeration)?;
    let candidate = devices
        .filter(is_known_garmin_usb_device)
        .find(|device| usb_topology_location_id(device) == location_id)
        .ok_or(MtpError::UnsafeResetTarget(location_id))?;
    let device = candidate.open().await.map_err(|source| MtpError::UsbOpen {
        location_id,
        source,
    })?;
    device.reset().await.map_err(|source| MtpError::UsbReset {
        location_id,
        source,
    })?;
    drop(device);
    tokio::time::sleep(GARMIN_MTP_RESET_QUIET_PERIOD).await;
    Ok(())
}

fn is_known_garmin_usb_device(device: &nusb::DeviceInfo) -> bool {
    KNOWN_GARMIN_MTP.contains(&(device.vendor_id(), device.product_id()))
}

/// Reproduce the stable location identifier exposed by `mtp-rs` so the reset
/// targets the exact USB port selected by the user, not merely the first Garmin.
fn usb_topology_location_id(device: &nusb::DeviceInfo) -> u64 {
    topology_location_id(device.bus_id(), device.port_chain())
}

fn topology_location_id(bus_id: &str, port_chain: &[u8]) -> u64 {
    let mut hash = USB_TOPOLOGY_HASH_OFFSET;
    for byte in bus_id
        .bytes()
        .chain([USB_TOPOLOGY_FIELD_SEPARATOR])
        .chain(port_chain.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(USB_TOPOLOGY_HASH_PRIME);
    }
    hash
}

/// Inspect declared paths through an exclusive raw-MTP session.
/// # Errors
/// [`MtpError`] if the device cannot be opened, enumerated, or closed.
pub async fn inventory_mtp(
    location_id: u64,
    paths: &[SafeRelativePath],
) -> Result<DeviceInventory, MtpError> {
    let device = open_raw_mtp(location_id).await?;
    let result = inventory_open_mtp(&device, paths).await;
    let close = device.close().await;
    let paths = result?;
    close?;
    Ok(DeviceInventory {
        transport: TransportKind::Mtp,
        paths,
    })
}

async fn inventory_open_mtp(
    device: &MtpDevice,
    paths: &[SafeRelativePath],
) -> Result<Vec<DevicePathInspection>, MtpError> {
    let storages = device.storages().await?;
    let mut inspected = Vec::with_capacity(paths.len().saturating_mul(storages.len()));
    for (index, storage) in storages.iter().enumerate() {
        let storage_id = format!("{:016x}", storage.id().0);
        let storage_label = if storage.info().description.is_empty() {
            format!("MTP storage {}", index + 1)
        } else {
            storage.info().description.clone()
        };
        for path in paths {
            let (state, size) = inspect_mtp_path(storage, path).await?;
            inspected.push(DevicePathInspection {
                storage_id: storage_id.clone(),
                storage_label: storage_label.clone(),
                path: path.clone(),
                state,
                size,
            });
        }
    }
    Ok(inspected)
}

async fn inspect_mtp_path(
    storage: &Storage,
    relative: &SafeRelativePath,
) -> Result<(DevicePathState, Option<u64>), MtpError> {
    let components = relative
        .as_path()
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut parent = None;
    for (index, component) in components.iter().enumerate() {
        let matches = storage
            .list_objects(parent)
            .await?
            .into_iter()
            .filter(|object| object.filename.eq_ignore_ascii_case(component))
            .collect::<Vec<_>>();
        let object = match matches.as_slice() {
            [] => return Ok((DevicePathState::Missing, None)),
            [object] => object,
            _ => return Ok((DevicePathState::Ambiguous, None)),
        };
        let is_last = index + 1 == components.len();
        if !is_last && !object.is_folder() {
            return Ok((DevicePathState::Other, None));
        }
        if is_last {
            return Ok(if object.is_file() {
                (DevicePathState::RegularFile, Some(object.size))
            } else {
                (DevicePathState::Directory, None)
            });
        }
        parent = Some(object.handle);
    }
    Ok((DevicePathState::Other, None))
}

async fn read_manifest_with_garmin_opcode(location_id: u64) -> Result<String, MtpFastPathError> {
    let device = PtpDevice::open_by_location_with_timeout(location_id, GARMIN_MTP_METADATA_TIMEOUT)
        .await
        .map_err(MtpFastPathError::Connection)?;
    let session = device
        .open_session()
        .await
        .map_err(MtpFastPathError::Connection)?;
    let (response, payload) = session
        .execute_with_receive(OperationCode::Unknown(0x9001), &[])
        .await?;
    if response.code != ResponseCode::Ok || payload.len() < 4 {
        session.close().await?;
        return Err(MtpFastPathError::MissingObjectId);
    }
    let object_id = u32::from_le_bytes(
        payload[..4]
            .try_into()
            .map_err(|_| MtpFastPathError::MissingObjectId)?,
    );
    let bytes = session.get_object(PtpObjectHandle(object_id)).await?;
    session.close().await?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(MtpFastPathError::ManifestTooLarge);
    }
    String::from_utf8(bytes).map_err(Into::into)
}

/// Upload a local file under a unique name, verify its size, and delete it.
/// # Errors
/// [`MtpError`] if source I/O or any MTP operation fails.
pub async fn probe_mtp_file(
    location_id: u64,
    source: &Path,
    recovery_root: &Path,
    device_identity: &str,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let device = open_raw_mtp(location_id).await?;
    probe_mtp_file_with_device(
        location_id,
        device,
        source,
        recovery_root,
        device_identity,
        progress,
    )
    .await
}

async fn probe_mtp_file_with_device(
    location_id: u64,
    device: MtpDevice,
    source: &Path,
    recovery_root: &Path,
    device_identity: &str,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let prepared = prepare_probe(device, source, recovery_root, device_identity, progress).await?;
    let PreparedProbe { active, source } = prepared;
    let outcome = upload_mtp_file(
        &active.storage,
        Some(active.parent),
        &active.lease.receipt.object_name,
        source,
        active.bytes,
        RawMtpUploadProgress {
            reporter: progress.clone(),
            stage: OperationStage::Upload,
            label: "Uploading disposable map object",
            completed_before: 0,
            total: active.bytes,
            path: None,
            complete_payload_stage: true,
        },
    )
    .await;
    match outcome {
        RawMtpUploadOutcome::Acknowledged {
            handle,
            payload_elapsed,
            finalize_elapsed,
        } => {
            complete_acknowledged_probe(
                active,
                handle,
                ProbeTiming::new(payload_elapsed, finalize_elapsed),
                progress,
            )
            .await
        }
        RawMtpUploadOutcome::Ambiguous {
            handle,
            payload_elapsed,
            finalize_elapsed,
            error,
        } => {
            let timing = ProbeTiming::new(payload_elapsed, finalize_elapsed);
            reconcile_ambiguous_probe(location_id, active, handle, timing, error, progress).await
        }
        RawMtpUploadOutcome::Failed { handle, error } => {
            let operation = if progress.is_cancelled() {
                MtpError::Cancelled
            } else {
                MtpError::Upload(error)
            };
            cleanup_after_probe_error(active, handle, operation, progress).await
        }
    }
}

struct PreparedProbe {
    active: ActiveProbe,
    source: tokio::fs::File,
}

struct ActiveProbe {
    device: MtpDevice,
    storage: Storage,
    parent: ObjectHandle,
    lease: ProbeLease,
    bytes: u64,
    expected_sha256: String,
}

#[derive(Clone, Copy)]
struct ProbeTiming {
    upload_seconds: f64,
    device_finalize_seconds: f64,
}

impl ProbeTiming {
    fn new(upload: Duration, finalize: Duration) -> Self {
        Self {
            upload_seconds: upload.as_secs_f64(),
            device_finalize_seconds: finalize.as_secs_f64(),
        }
    }
}

async fn prepare_probe(
    device: MtpDevice,
    source: &Path,
    recovery_root: &Path,
    device_identity: &str,
    progress: &ProgressReporter,
) -> Result<PreparedProbe, MtpError> {
    let bytes = tokio::fs::metadata(source).await?.len();
    let expected_sha256 = hash_probe_source(source, bytes).await?;
    let source = tokio::fs::File::open(source).await?;
    let storages = match device.storages().await {
        Ok(storages) => {
            progress.device_state(Ok(mtp_state(&storages)));
            storages
        }
        Err(error) => {
            progress.device_state(Err(error.to_string()));
            return finish_device(device, Err(error.into())).await;
        }
    };
    if let Err(error) =
        reconcile_probe_receipts(&storages, recovery_root, device_identity, progress).await
    {
        return finish_device(device, Err(error)).await;
    }
    let (storage, parent) = match select_probe_storage(storages, bytes).await {
        Ok(selected) => selected,
        Err(error) => return finish_device(device, Err(error)).await,
    };
    let lease = match ProbeLease::create(
        recovery_root,
        device_identity,
        storage.id().0,
        bytes,
        expected_sha256.clone(),
    )
    .await
    {
        Ok(lease) => lease,
        Err(error) => return finish_device(device, Err(error)).await,
    };
    Ok(PreparedProbe {
        active: ActiveProbe {
            device,
            storage,
            parent,
            lease,
            bytes,
            expected_sha256,
        },
        source,
    })
}

async fn complete_acknowledged_probe(
    active: ActiveProbe,
    handle: ObjectHandle,
    timing: ProbeTiming,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let read_back_seconds = match verify_probe_file(
        &active.storage,
        handle,
        active.bytes,
        &active.expected_sha256,
        progress,
    )
    .await
    {
        Ok(elapsed) => elapsed,
        Err(error) => {
            return cleanup_after_probe_error(active, Some(handle), error, progress).await;
        }
    };
    finish_verified_probe(
        active,
        handle,
        timing,
        UploadCompletion::Acknowledged,
        read_back_seconds,
        progress,
    )
    .await
}

async fn reconcile_ambiguous_probe(
    location_id: u64,
    mut active: ActiveProbe,
    partial_handle: ObjectHandle,
    timing: ProbeTiming,
    upload_error: mtp_rs::UploadError,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let upload_error = upload_error.to_string();
    let mut last_error = ProbeRecoveryReason::Detail(upload_error.clone());
    for delay in MTP_RECOVERY_POLICY.delays() {
        wait_for_device_reconciliation(progress, delay).await;
        let object =
            match find_probe_object(&active.storage, &active.lease.receipt.object_name).await {
                Ok(object) => object,
                Err(error) => {
                    last_error = ProbeRecoveryReason::from_mtp_error(&error);
                    if session_requires_reopen(&error) {
                        return reopen_ambiguous_probe(
                            location_id,
                            active,
                            timing,
                            &upload_error,
                            progress,
                        )
                        .await;
                    }
                    continue;
                }
            };
        let Some((parent, handle)) = object else {
            let ActiveProbe { device, lease, .. } = active;
            lease.remove().await?;
            let error = MtpError::UploadReconciliation(format!(
                "{upload_error}; uploaded object {partial_handle:?} is absent after session recovery"
            ));
            return finish_device(device, Err(error)).await;
        };
        active.parent = parent;
        let read_back = verify_probe_file(
            &active.storage,
            handle,
            active.lease.receipt.bytes,
            &active.lease.receipt.sha256,
            progress,
        )
        .await;
        match read_back {
            Ok(read_back_seconds) => {
                progress.completed(
                    OperationStage::DeviceFinalize,
                    "MTP upload reconciled by SHA-256 read-back",
                    1,
                    Some(1),
                );
                return finish_verified_probe(
                    active,
                    handle,
                    timing,
                    UploadCompletion::ReconciledAfterTimeout,
                    read_back_seconds,
                    progress,
                )
                .await;
            }
            Err(error @ (MtpError::ProbeSize { .. } | MtpError::ProbeChecksum)) => {
                let operation = MtpError::UploadReconciliation(format!(
                    "{upload_error}; uploaded object did not verify: {error}"
                ));
                return cleanup_after_probe_error(active, Some(handle), operation, progress).await;
            }
            Err(MtpError::Cancelled) => {
                return cleanup_after_probe_error(
                    active,
                    Some(handle),
                    MtpError::Cancelled,
                    progress,
                )
                .await;
            }
            Err(error) => {
                last_error = ProbeRecoveryReason::from_mtp_error(&error);
                if session_requires_reopen(&error) {
                    return reopen_ambiguous_probe(
                        location_id,
                        active,
                        timing,
                        &upload_error,
                        progress,
                    )
                    .await;
                }
            }
        }
    }
    let receipt = active.lease.path.clone();
    drop(active);
    Err(MtpError::ProbeRecoveryPending {
        receipt,
        reason: last_error
            .with_context(format!("{upload_error}; in-session reconciliation failed")),
    })
}

fn session_requires_reopen(error: &MtpError) -> bool {
    matches!(
        error,
        MtpError::Protocol(mtp_rs::Error::DeviceReset | mtp_rs::Error::Disconnected)
    )
}

async fn reopen_ambiguous_probe(
    location_id: u64,
    active: ActiveProbe,
    timing: ProbeTiming,
    upload_error: &str,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let ActiveProbe {
        device,
        storage,
        lease,
        ..
    } = active;
    drop(storage);
    drop(device);
    reconcile_reopened_probe(location_id, lease, timing, upload_error, progress).await
}

async fn reconcile_reopened_probe(
    location_id: u64,
    lease: ProbeLease,
    timing: ProbeTiming,
    upload_error: &str,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let mut last_error = ProbeRecoveryReason::Detail("session must be reopened".to_owned());
    for delay in MTP_RECOVERY_POLICY.delays() {
        wait_for_device_reopen(progress, delay).await;
        let reopened = match reopen_probe_object(location_id, &lease).await {
            Ok(reopened) => reopened,
            Err(error) => {
                last_error = ProbeRecoveryReason::from_mtp_error(&error);
                continue;
            }
        };
        let Some((storage, parent, handle)) = reopened.object else {
            lease.remove().await?;
            let error = MtpError::UploadReconciliation(format!(
                "{upload_error}; uploaded object absent after reopening"
            ));
            return finish_device(reopened.device, Err(error)).await;
        };
        let read_back = verify_probe_file(
            &storage,
            handle,
            lease.receipt.bytes,
            &lease.receipt.sha256,
            progress,
        )
        .await;
        match read_back {
            Ok(read_back_seconds) => {
                progress.completed(
                    OperationStage::DeviceFinalize,
                    "MTP upload reconciled by SHA-256 read-back",
                    1,
                    Some(1),
                );
                let active = active_from_reopened(reopened.device, storage, parent, lease);
                return finish_verified_probe(
                    active,
                    handle,
                    timing,
                    UploadCompletion::ReconciledAfterTimeout,
                    read_back_seconds,
                    progress,
                )
                .await;
            }
            Err(error @ (MtpError::ProbeSize { .. } | MtpError::ProbeChecksum)) => {
                let operation = MtpError::UploadReconciliation(format!(
                    "{upload_error}; uploaded object did not verify: {error}"
                ));
                let active = active_from_reopened(reopened.device, storage, parent, lease);
                return cleanup_after_probe_error(active, Some(handle), operation, progress).await;
            }
            Err(MtpError::Cancelled) => {
                let active = active_from_reopened(reopened.device, storage, parent, lease);
                return cleanup_after_probe_error(
                    active,
                    Some(handle),
                    MtpError::Cancelled,
                    progress,
                )
                .await;
            }
            Err(error) => {
                last_error = ProbeRecoveryReason::from_mtp_error(&error);
                drop(storage);
                drop(reopened.device);
            }
        }
    }
    Err(MtpError::ProbeRecoveryPending {
        receipt: lease.path,
        reason: last_error.with_context(format!(
            "{upload_error}; disconnected-session reconciliation failed"
        )),
    })
}

async fn wait_for_device_reconciliation(progress: &ProgressReporter, delay: Duration) {
    progress.advanced(
        OperationStage::DeviceFinalize,
        format!(
            "Waiting {}s for the device before reconciling the upload",
            delay.as_secs()
        ),
        0,
        None,
    );
    tokio::time::sleep(delay).await;
}

async fn wait_for_device_reopen(progress: &ProgressReporter, delay: Duration) {
    progress.advanced(
        OperationStage::DeviceFinalize,
        format!(
            "Session ended; waiting {}s before reopening the device",
            delay.as_secs()
        ),
        0,
        None,
    );
    tokio::time::sleep(delay).await;
}

struct ReopenedProbe {
    device: MtpDevice,
    object: Option<(Storage, ObjectHandle, ObjectHandle)>,
}

async fn reopen_probe_object(
    location_id: u64,
    lease: &ProbeLease,
) -> Result<ReopenedProbe, MtpError> {
    let device = open_raw_mtp(location_id).await?;
    let storages = device.storages().await?;
    let storage = storages
        .into_iter()
        .find(|storage| storage.id().0 == lease.receipt.storage_id)
        .ok_or_else(|| {
            MtpError::UploadReconciliation("recorded MTP storage is unavailable".to_owned())
        })?;
    let object = find_probe_object(&storage, &lease.receipt.object_name)
        .await?
        .map(|(parent, handle)| (storage, parent, handle));
    Ok(ReopenedProbe { device, object })
}

fn active_from_reopened(
    device: MtpDevice,
    storage: Storage,
    parent: ObjectHandle,
    lease: ProbeLease,
) -> ActiveProbe {
    ActiveProbe {
        device,
        storage,
        parent,
        bytes: lease.receipt.bytes,
        expected_sha256: lease.receipt.sha256.clone(),
        lease,
    }
}

async fn finish_verified_probe(
    active: ActiveProbe,
    handle: ObjectHandle,
    timing: ProbeTiming,
    completion: UploadCompletion,
    read_back_seconds: f64,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    let cleanup_seconds = delete_probe_file(
        &active.storage,
        active.parent,
        &active.lease.receipt.object_name,
        handle,
        progress,
    )
    .await
    .map_err(|error| MtpError::ProbeRecoveryPending {
        receipt: active.lease.path.clone(),
        reason: error.to_string().into(),
    })?;
    active.lease.remove().await?;
    active.device.close().await?;
    Ok(DeviceProbeReport {
        transport: TransportKind::Mtp,
        bytes: active.bytes,
        upload_seconds: timing.upload_seconds,
        device_finalize_seconds: timing.device_finalize_seconds,
        upload_completion: completion,
        read_back_seconds,
        cleanup_seconds,
        content_verified: true,
        temporary_file_removed: true,
    })
}

async fn cleanup_after_probe_error(
    active: ActiveProbe,
    handle: Option<ObjectHandle>,
    operation: MtpError,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MtpError> {
    if let Some(handle) = handle
        && let Err(cleanup) = delete_probe_file(
            &active.storage,
            active.parent,
            &active.lease.receipt.object_name,
            handle,
            progress,
        )
        .await
    {
        return Err(MtpError::ProbeRecoveryPending {
            receipt: active.lease.path,
            reason: format!("{operation}; cleanup failed: {cleanup}").into(),
        });
    }
    active.lease.remove().await?;
    finish_device(active.device, Err(operation)).await
}

async fn finish_device<T>(
    device: MtpDevice,
    operation: Result<T, MtpError>,
) -> Result<T, MtpError> {
    let close = device.close().await;
    match (operation, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Ok(())) => Err(error),
        (Err(operation), Err(cleanup)) => Err(MtpError::SessionCleanup {
            operation: operation.to_string(),
            cleanup,
        }),
    }
}

async fn select_probe_storage(
    storages: Vec<Storage>,
    bytes: u64,
) -> Result<(Storage, ObjectHandle), MtpError> {
    let mut selected = None;
    for storage in storages {
        if !storage.info().is_writable || storage.info().free_space < bytes {
            continue;
        }
        let roots = storage.list_objects(None).await?;
        if let Some(garmin) = roots
            .iter()
            .find(|item| item.is_folder() && item.filename.eq_ignore_ascii_case("Garmin"))
        {
            selected = Some((storage, garmin.handle));
            break;
        }
    }
    selected.ok_or(MtpError::NoWritableGarminStorage)
}

async fn reconcile_probe_receipts(
    storages: &[Storage],
    root: &Path,
    device_identity: &str,
    progress: &ProgressReporter,
) -> Result<(), MtpError> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let contents = tokio::fs::read(entry.path()).await?;
        let receipt: ProbeReceipt =
            serde_json::from_slice(&contents).map_err(std::io::Error::other)?;
        if receipt.device_identity != device_identity {
            continue;
        }
        if receipt.version != PROBE_RECEIPT_VERSION {
            return Err(MtpError::ProbeRecoveryPending {
                receipt: entry.path(),
                reason: format!("unsupported recovery receipt version {}", receipt.version).into(),
            });
        }
        let lease = ProbeLease {
            path: entry.path(),
            receipt,
        };
        reconcile_probe_receipt(storages, lease, progress).await?;
    }
    Ok(())
}

async fn reconcile_probe_receipt(
    storages: &[Storage],
    lease: ProbeLease,
    progress: &ProgressReporter,
) -> Result<(), MtpError> {
    let storage = storages
        .iter()
        .find(|storage| storage.id().0 == lease.receipt.storage_id)
        .ok_or_else(|| MtpError::ProbeRecoveryPending {
            receipt: lease.path.clone(),
            reason: "recorded MTP storage is unavailable".into(),
        })?;
    let Some((parent, handle)) = find_probe_object(storage, &lease.receipt.object_name).await?
    else {
        lease.remove().await?;
        return Ok(());
    };
    progress.started(
        OperationStage::DeviceFinalize,
        "Reconciling a disposable object from an earlier benchmark",
        None,
    );
    let verification = verify_probe_file(
        storage,
        handle,
        lease.receipt.bytes,
        &lease.receipt.sha256,
        progress,
    )
    .await;
    let verified = verification.is_ok();
    if let Err(error) = delete_probe_file(
        storage,
        parent,
        &lease.receipt.object_name,
        handle,
        progress,
    )
    .await
    {
        return Err(MtpError::ProbeRecoveryPending {
            receipt: lease.path,
            reason: error.to_string().into(),
        });
    }
    lease.remove().await?;
    progress.completed(
        OperationStage::DeviceFinalize,
        if verified {
            "Verified and removed an earlier disposable MTP object"
        } else {
            "Removed an incomplete disposable MTP object"
        },
        1,
        Some(1),
    );
    Ok(())
}

async fn find_probe_object(
    storage: &Storage,
    object_name: &str,
) -> Result<Option<(ObjectHandle, ObjectHandle)>, MtpError> {
    let roots = storage.list_objects(None).await?;
    let Some(garmin) = roots
        .iter()
        .find(|item| item.is_folder() && item.filename.eq_ignore_ascii_case("Garmin"))
    else {
        return Ok(None);
    };
    let mut matches = storage
        .list_objects(Some(garmin.handle))
        .await?
        .into_iter()
        .filter(|object| object.filename == object_name);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(MtpError::AmbiguousProbeObject(object_name.to_owned()));
    }
    Ok(first.map(|object| (garmin.handle, object.handle)))
}

fn mtp_state(storages: &[Storage]) -> crate::DeviceStateSnapshot {
    crate::DeviceStateSnapshot {
        storages: storages
            .iter()
            .enumerate()
            .map(|(index, storage)| {
                let info = storage.info();
                crate::DeviceStorageState {
                    id: format!("{:016x}", storage.id().0),
                    label: if info.description.is_empty() {
                        format!("MTP storage {}", index + 1)
                    } else {
                        info.description.clone()
                    },
                    capacity: crate::StorageCapacity::new(info.total_capacity, info.free_space),
                    writable: Some(info.is_writable),
                }
            })
            .collect(),
    }
}

async fn verify_probe_file(
    storage: &Storage,
    handle: ObjectHandle,
    bytes: u64,
    expected_sha256: &str,
    progress: &ProgressReporter,
) -> Result<f64, MtpError> {
    progress.started(
        OperationStage::DeviceVerify,
        "Reading MTP object back",
        Some(bytes),
    );
    let actual = match storage.get_object_info(handle).await {
        Ok(info) => info.size,
        Err(error) => {
            progress.failed(OperationStage::DeviceVerify, "MTP object inspection failed");
            return Err(MtpError::Protocol(error));
        }
    };
    if actual != bytes {
        progress.failed(OperationStage::DeviceVerify, "MTP object size mismatch");
        return Err(MtpError::ProbeSize {
            expected: bytes,
            actual,
        });
    }
    let verify_started = Instant::now();
    let mut download = match storage.download(handle, ByteRange::Full).await {
        Ok(download) => download,
        Err(error) => {
            progress.failed(OperationStage::DeviceVerify, "MTP read-back failed");
            return Err(MtpError::Protocol(error));
        }
    };
    let mut actual = 0_u64;
    let mut hash = Sha256::new();
    while let Some(chunk) = download.next_chunk().await {
        if progress.is_cancelled() {
            if let Err(error) = download.cancel(mtp_rs::DEFAULT_CANCEL_TIMEOUT).await {
                progress.failed(
                    OperationStage::DeviceVerify,
                    "MTP read-back cancellation failed",
                );
                return Err(MtpError::Protocol(error));
            }
            progress.failed(OperationStage::DeviceVerify, "MTP read-back cancelled");
            return Err(MtpError::Cancelled);
        }
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                progress.failed(OperationStage::DeviceVerify, "MTP read-back failed");
                return Err(MtpError::Protocol(error));
            }
        };
        actual = actual
            .checked_add(u64::try_from(chunk.len()).map_err(std::io::Error::other)?)
            .ok_or(MtpError::SizeOverflow)?;
        if actual > bytes {
            progress.failed(OperationStage::DeviceVerify, "MTP read-back size mismatch");
            return Err(MtpError::ProbeSize {
                expected: bytes,
                actual,
            });
        }
        hash.update(&chunk);
        progress.advanced(
            OperationStage::DeviceVerify,
            "Reading MTP object back",
            actual,
            Some(bytes),
        );
    }
    if actual != bytes {
        progress.failed(OperationStage::DeviceVerify, "MTP read-back size mismatch");
        return Err(MtpError::ProbeSize {
            expected: bytes,
            actual,
        });
    }
    if hex::encode(hash.finalize()) != expected_sha256 {
        progress.failed(
            OperationStage::DeviceVerify,
            "MTP read-back checksum mismatch",
        );
        return Err(MtpError::ProbeChecksum);
    }
    let read_back_seconds = verify_started.elapsed().as_secs_f64();
    progress.completed(
        OperationStage::DeviceVerify,
        "Read back and SHA-256 verified",
        actual,
        Some(bytes),
    );
    Ok(read_back_seconds)
}

async fn hash_probe_source(source: &Path, expected: u64) -> Result<String, MtpError> {
    let mut file = tokio::fs::File::open(source).await?;
    let mut actual = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        actual = actual
            .checked_add(u64::try_from(count).map_err(std::io::Error::other)?)
            .ok_or(MtpError::SizeOverflow)?;
        if actual > expected {
            return Err(MtpError::ProbeSize { expected, actual });
        }
        hash.update(&buffer[..count]);
    }
    if actual != expected {
        return Err(MtpError::ProbeSize { expected, actual });
    }
    Ok(hex::encode(hash.finalize()))
}

async fn delete_probe_file(
    storage: &Storage,
    parent: ObjectHandle,
    object_name: &str,
    handle: ObjectHandle,
    progress: &ProgressReporter,
) -> Result<f64, MtpError> {
    progress.started(
        OperationStage::Delete,
        "Removing disposable MTP object",
        None,
    );
    let delete_started = Instant::now();
    if let Err(error) = storage.delete(handle).await {
        progress.failed(OperationStage::Delete, "MTP object deletion failed");
        return Err(MtpError::Protocol(error));
    }
    if storage
        .list_objects(Some(parent))
        .await?
        .iter()
        .any(|object| object.filename == object_name)
    {
        progress.failed(OperationStage::Delete, "Disposable MTP object still exists");
        return Err(MtpError::ProbeNotRemoved);
    }
    let deletion_seconds = delete_started.elapsed().as_secs_f64();
    progress.completed(
        OperationStage::Delete,
        "Disposable MTP object removed",
        1,
        Some(1),
    );
    Ok(deletion_seconds)
}

#[derive(Debug, Error)]
pub enum ProbeRecoveryReason {
    #[error("{0}")]
    Detail(String),
    #[error(
        "{context}: the USB interface remained busy while reopening; its owner was not identified"
    )]
    InterfaceBusy { context: String },
}

impl ProbeRecoveryReason {
    fn from_mtp_error(error: &MtpError) -> Self {
        if error.is_interface_busy() {
            Self::InterfaceBusy {
                context: "device session recovery failed".to_owned(),
            }
        } else {
            Self::Detail(error.to_string())
        }
    }

    fn with_context(self, context: String) -> Self {
        match self {
            Self::Detail(detail) => Self::Detail(format!("{context}: {detail}")),
            Self::InterfaceBusy { .. } => Self::InterfaceBusy { context },
        }
    }

    #[must_use]
    pub const fn is_interface_busy(&self) -> bool {
        matches!(self, Self::InterfaceBusy { .. })
    }
}

impl From<String> for ProbeRecoveryReason {
    fn from(detail: String) -> Self {
        Self::Detail(detail)
    }
}

impl From<&str> for ProbeRecoveryReason {
    fn from(detail: &str) -> Self {
        Self::Detail(detail.to_owned())
    }
}

#[derive(Debug, Error)]
pub enum MtpError {
    #[error("MTP operation failed: {0}")]
    Protocol(#[from] mtp_rs::Error),
    #[error("MTP manifest is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("Garmin/GarminDevice.xml was not found on any MTP storage")]
    MissingManifest,
    #[error("no writable MTP storage with a Garmin folder has sufficient space")]
    NoWritableGarminStorage,
    #[error("MTP upload failed: {0}")]
    Upload(#[from] mtp_rs::UploadError),
    #[error("MTP operation failed ({operation}); session cleanup also failed ({cleanup})")]
    SessionCleanup {
        operation: String,
        cleanup: mtp_rs::Error,
    },
    #[error("requested MTP transfer size cannot be represented on this host")]
    SizeOverflow,
    #[error("probe source I/O failed: {0}")]
    ProbeIo(#[from] std::io::Error),
    #[error("MTP probe size mismatch: expected {expected}, observed {actual}")]
    ProbeSize { expected: u64, actual: u64 },
    #[error("MTP probe read-back checksum did not match the source")]
    ProbeChecksum,
    #[error("disposable MTP object still exists after deletion")]
    ProbeNotRemoved,
    #[error("MTP probe recovery remains pending at {receipt}: {reason}")]
    ProbeRecoveryPending {
        receipt: PathBuf,
        reason: ProbeRecoveryReason,
    },
    #[error("multiple MTP objects matched the disposable probe name {0}")]
    AmbiguousProbeObject(String),
    #[error("MTP upload completion could not be reconciled: {0}")]
    UploadReconciliation(String),
    #[error("MTP probe cancelled")]
    Cancelled,
    #[error("refusing to reset non-Garmin MTP location {0}")]
    UnsafeResetTarget(u64),
    #[error("could not enumerate USB devices for Garmin recovery")]
    UsbEnumeration(#[source] nusb::Error),
    #[error("could not open Garmin USB location {location_id} for recovery")]
    UsbOpen {
        location_id: u64,
        #[source]
        source: nusb::Error,
    },
    #[error("whole-device USB reset failed for Garmin location {location_id}")]
    UsbReset {
        location_id: u64,
        #[source]
        source: nusb::Error,
    },
}

impl MtpError {
    #[must_use]
    pub fn is_interface_busy(&self) -> bool {
        match self {
            Self::Protocol(error) => error.is_exclusive_access(),
            Self::Upload(error) => error.source.is_exclusive_access(),
            Self::SessionCleanup { cleanup, .. } => cleanup.is_exclusive_access(),
            Self::ProbeRecoveryPending { reason, .. } => reason.is_interface_busy(),
            _ => false,
        }
    }

    /// Whether a bounded open proved that this raw transport needs a Garmin-only USB reset.
    #[must_use]
    pub const fn needs_transport_reset(&self) -> bool {
        matches!(
            self,
            Self::Protocol(mtp_rs::Error::Timeout | mtp_rs::Error::DeviceReset)
        )
    }
}

#[derive(Debug, Error)]
enum MtpFastPathError {
    #[error("Garmin MTP connection failed: {0}")]
    Connection(mtp_rs::PtpError),
    #[error("Garmin MTP operation failed: {0}")]
    Protocol(#[from] mtp_rs::PtpError),
    #[error("Garmin MTP extension returned no manifest object ID")]
    MissingObjectId,
    #[error("GarminDevice.xml exceeds 4 MB")]
    ManifestTooLarge,
    #[error("MTP manifest is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[cfg(test)]
mod tests {
    use super::{
        FENIX_8_SOLAR_47_MTP_PRODUCT_ID, GARMIN_USB_VENDOR_ID, KNOWN_GARMIN_MTP, MtpError,
        topology_location_id, usb_reset_known_ineffective,
    };

    #[test]
    fn garmin_protocol_mode_is_not_an_mtp_candidate() {
        assert!(!KNOWN_GARMIN_MTP.contains(&(GARMIN_USB_VENDOR_ID, 0x0003)));
    }

    #[test]
    fn only_an_unresponsive_transport_requests_a_usb_reset() {
        assert!(MtpError::Protocol(mtp_rs::Error::Timeout).needs_transport_reset());
        assert!(MtpError::Protocol(mtp_rs::Error::DeviceReset).needs_transport_reset());
        assert!(!MtpError::Protocol(mtp_rs::Error::ExclusiveAccess).needs_transport_reset());
        assert!(!MtpError::Protocol(mtp_rs::Error::PermissionDenied).needs_transport_reset());
    }

    #[test]
    fn topology_location_matches_retained_fenix_port_identity() {
        assert_eq!(topology_location_id("005", &[1]), 7_084_020_013_999_254_044);
    }

    #[test]
    fn retained_fenix_is_not_subjected_to_a_disproved_usb_reset() {
        assert!(usb_reset_known_ineffective(
            GARMIN_USB_VENDOR_ID,
            FENIX_8_SOLAR_47_MTP_PRODUCT_ID,
        ));
        assert!(!usb_reset_known_ineffective(GARMIN_USB_VENDOR_ID, 0x5158,));
    }
}
