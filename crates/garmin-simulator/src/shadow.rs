//! Retained device snapshots used by runtime simulation and transaction tests.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use garmin_capture::SessionCapture;
use garmin_device::storage::{DeviceRead, DeviceWrite};
use garmin_device::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState,
    MountedMtpBackupProgress, SafeRelativePath,
};
use garmin_model::map::MapAuthorization;
use garmin_progress::{OperationStage, ProgressReporter};
use garmin_update::UpdatePlan;
use indoc::indoc;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;

use crate::virtual_device::{VirtualDevice, VirtualVolume};

mod device;
mod report;
pub use report::{
    ChangeKind, FileState, ObservedChange, PlannedChange, SimulationReport, SimulationStatus,
};

#[derive(Debug, Serialize, Deserialize)]
struct Volume {
    key: String,
    source_id: String,
    label: String,
    #[serde(default)]
    capacity: Option<garmin_device::StorageCapacity>,
    #[serde(default)]
    writable: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotFile {
    storage: String,
    path: SafeRelativePath,
    planned: PlannedChange,
    before: FileState,
}

#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    version: u8,
    execution_target: String,
    source_device: String,
    plan_digest: String,
    primary: String,
    volumes: Vec<Volume>,
    files: Vec<SnapshotFile>,
}

impl Snapshot {
    async fn verify_source(
        &self,
        source: &dyn DeviceRead,
        paths: &[SafeRelativePath],
        inventory: &DeviceInventory,
    ) -> Result<()> {
        if source.inventory(paths).await? != *inventory {
            bail!("source inventory changed while preparing the virtual device");
        }
        for entry in &self.files {
            if let FileState::File { bytes, sha256 } = &entry.before {
                let volume = self
                    .volumes
                    .iter()
                    .find(|volume| volume.key == entry.storage)
                    .context("snapshot storage is missing")?;
                source
                    .verify(&volume.source_id, &entry.path, *bytes, sha256)
                    .await?;
            }
        }
        Ok(())
    }
}

/// Owns registration while the capture owns all backing files.
pub struct Shadow {
    registered: VirtualDevice,
    snapshot: Snapshot,
}

impl Shadow {
    /// Copy affected source state through read-only capabilities.
    ///
    /// # Errors
    /// Unsafe, incomplete, or changed inventory; capture or registration failure.
    pub async fn create(
        source: &dyn DeviceRead,
        manifest: &DeviceManifest,
        plan: &UpdatePlan,
        authorization: &MapAuthorization,
        capture: &SessionCapture,
        progress: &ProgressReporter,
    ) -> Result<Self> {
        capture
            .write_bytes(Path::new("simulation/README.md"), README.as_bytes())
            .await?;
        let paths = snapshot_scope(plan, authorization)?;
        let requested = paths
            .values()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let inventory = source.inventory(&requested).await?;
        let source_state = source.state().await?;
        capture
            .write_json(Path::new("simulation/source-state.json"), &source_state)
            .await?;
        let primary = source.primary_storage_id().await?;
        let volumes = snapshot_volumes(&inventory, &source_state, &primary)?;
        let mut snapshot = Snapshot {
            version: 1,
            execution_target: format!("virtual:{}", uuid::Uuid::new_v4()),
            source_device: manifest.identity_digest(),
            plan_digest: plan.digest.clone(),
            primary,
            volumes,
            files: Vec::new(),
        };
        let total = inventory.paths.iter().try_fold(0_u64, |sum, item| {
            sum.checked_add(item.size.unwrap_or(0))
                .context("snapshot size overflow")
        })?;
        garmin_update::space::check_host_space(
            capture.root(),
            total.checked_mul(2).context("snapshot size overflow")?,
        )
        .await?;
        progress.started(
            OperationStage::Backup,
            "Copying source files into an isolated virtual device",
            Some(total),
        );
        capture
            .write_json(Path::new("simulation/source-inventory.json"), &inventory)
            .await?;
        let mut copied = 0_u64;
        for volume in &snapshot.volumes {
            capture
                .create_directory(&PathBuf::from("simulation/device").join(&volume.key))
                .await?;
            capture
                .create_directory(&PathBuf::from("simulation/before").join(&volume.key))
                .await?;
            for (path, planned) in paths.values() {
                let matches = inventory
                    .paths
                    .iter()
                    .filter(|item| item.storage_id == volume.source_id && item.path == *path)
                    .collect::<Vec<_>>();
                let [item] = matches.as_slice() else {
                    bail!("source inventory is incomplete or duplicated for {path}");
                };
                let before =
                    copy_original(source, item, volume, capture, progress, copied, total).await?;
                if let FileState::File { bytes, .. } = &before {
                    copied += bytes;
                }
                snapshot.files.push(SnapshotFile {
                    storage: volume.key.clone(),
                    path: path.clone(),
                    planned: *planned,
                    before,
                });
            }
        }
        snapshot
            .verify_source(source, &requested, &inventory)
            .await?;
        capture
            .write_json(Path::new("simulation/snapshot.json"), &snapshot)
            .await?;
        let registered = register(capture.root(), &snapshot)?;
        progress.completed(
            OperationStage::Backup,
            "Source snapshot retained; physical device remains read-only",
            copied,
            Some(total),
        );
        Ok(Self {
            registered,
            snapshot,
        })
    }

    #[must_use]
    pub fn device(&self) -> &dyn DeviceWrite {
        self
    }

    /// Reopen retained state; no physical-device access or new snapshot.
    ///
    /// # Errors
    /// Invalid snapshot, unsafe backing paths, or registration failure.
    pub async fn reopen(capture_root: &Path) -> Result<Self> {
        let snapshot: Snapshot = serde_json::from_slice(
            &tokio::fs::read(capture_root.join("simulation/snapshot.json")).await?,
        )?;
        if snapshot.version != 1 || !snapshot.execution_target.starts_with("virtual:") {
            bail!("unsupported simulation snapshot");
        }
        let registered = register(capture_root, &snapshot)?;
        Ok(Self {
            registered,
            snapshot,
        })
    }

    /// Compare actual backing files with captured originals, including failed runs.
    ///
    /// # Errors
    /// Capture-report persistence failure.
    pub async fn finish(
        &self,
        capture: &SessionCapture,
        succeeded: bool,
    ) -> Result<SimulationReport> {
        let report = self.observe_changes(capture.root(), succeeded).await;
        capture
            .write_json(Path::new("simulation/changes.json"), &report)
            .await?;
        let summary = report.markdown()?;
        capture
            .write_bytes(Path::new("simulation/summary.md"), summary.as_bytes())
            .await?;
        Ok(report)
    }

    async fn observe_changes(&self, root: &Path, succeeded: bool) -> SimulationReport {
        let mut changes = Vec::new();
        for entry in &self.snapshot.files {
            let file = root
                .join("simulation/device")
                .join(&entry.storage)
                .join(entry.path.as_path());
            let after = observe(&file).await;
            let observed = ChangeKind::between(&entry.before, &after);
            changes.push(ObservedChange {
                storage: entry.storage.clone(),
                path: entry.path.clone(),
                planned: entry.planned,
                observed,
                before: entry.before.clone(),
                after,
            });
        }
        let observed_all = changes
            .iter()
            .all(|entry| !matches!(entry.after, FileState::Unobserved { .. }));
        let status = if succeeded && observed_all {
            SimulationStatus::Complete
        } else {
            SimulationStatus::Incomplete
        };
        SimulationReport {
            version: 1,
            execution_target: self.snapshot.execution_target.clone(),
            source_device: self.snapshot.source_device.clone(),
            plan_digest: self.snapshot.plan_digest.clone(),
            status,
            physical_device_modified: false,
            changes,
        }
    }
}

fn snapshot_volumes(
    inventory: &DeviceInventory,
    state: &garmin_device::DeviceStateSnapshot,
    primary: &str,
) -> Result<Vec<Volume>> {
    let storages = inventory
        .paths
        .iter()
        .map(|item| (item.storage_id.clone(), item.storage_label.clone()))
        .collect::<BTreeMap<_, _>>();
    if !storages.contains_key(primary) {
        bail!("source inventory omitted the canonical storage");
    }
    storages
        .into_iter()
        .enumerate()
        .map(|(index, (source_id, label))| {
            let states = state
                .storages
                .iter()
                .filter(|storage| storage.id == source_id)
                .collect::<Vec<_>>();
            let [state] = states.as_slice() else {
                bail!("source storage metadata is missing or ambiguous");
            };
            Ok(Volume {
                key: format!("storage-{:03}", index + 1),
                source_id,
                label,
                capacity: Some(state.capacity.clone()),
                writable: state.writable,
            })
        })
        .collect()
}

async fn copy_original(
    source: &dyn DeviceRead,
    item: &DevicePathInspection,
    volume: &Volume,
    capture: &SessionCapture,
    progress: &ProgressReporter,
    copied: u64,
    total: u64,
) -> Result<FileState> {
    match (item.state, item.size) {
        (DevicePathState::Missing, None) => Ok(FileState::Absent),
        (DevicePathState::RegularFile, Some(bytes)) => {
            if progress.is_cancelled() {
                return Err(garmin_device::storage::DeviceIoError::Cancelled.into());
            }
            let before = PathBuf::from("simulation/before")
                .join(&volume.key)
                .join(item.path.as_path());
            let sink = garmin_device::BackupDestination::new(
                capture.artifact_path(&before)?,
                capture.create_file(&before).await?.into_std().await,
            )?;
            let sha256 = source
                .backup(
                    &item.storage_id,
                    &item.path,
                    bytes,
                    sink,
                    MountedMtpBackupProgress {
                        reporter: progress.clone(),
                        completed_before: copied,
                        total,
                    },
                )
                .await?;
            let expected = FileState::File { bytes, sha256 };
            if observe(&capture.root().join(&before)).await != expected {
                bail!("snapshot backup verification failed for {}", item.path);
            }
            let destination = PathBuf::from("simulation/device")
                .join(&volume.key)
                .join(item.path.as_path());
            capture
                .copy_file(&destination, &capture.root().join(before))
                .await?;
            if observe(&capture.root().join(destination)).await != expected {
                bail!("virtual-device seeding failed for {}", item.path);
            }
            Ok(expected)
        }
        _ => bail!("cannot snapshot {} in state {:?}", item.path, item.state),
    }
}

fn register(root: &Path, snapshot: &Snapshot) -> Result<VirtualDevice> {
    let mut volumes = Vec::new();
    for volume in &snapshot.volumes {
        if !volume.key.starts_with("storage-")
            || !volume.key[8..].chars().all(|ch| ch.is_ascii_digit())
        {
            bail!("unsafe snapshot storage key");
        }
        let directory = root.join("simulation/device").join(&volume.key);
        for ancestor in [
            root.join("simulation"),
            root.join("simulation/device"),
            directory.clone(),
        ] {
            if !std::fs::symlink_metadata(&ancestor)?.is_dir() {
                bail!("unsafe virtual-device directory");
            }
        }
        volumes.push(VirtualVolume {
            key: volume.source_id.clone(),
            label: volume.label.clone(),
            directory,
            capacity: snapshot
                .files
                .iter()
                .filter(|file| file.storage == volume.key)
                .try_fold(
                    volume
                        .capacity
                        .as_ref()
                        .and_then(garmin_device::StorageCapacity::bytes)
                        .map_or(0, |(_, free)| free),
                    |total, file| {
                        total
                            .checked_add(match file.before {
                                FileState::File { bytes, .. } => bytes,
                                _ => 0,
                            })
                            .context("virtual capacity overflow")
                    },
                )?,
            read_only: volume.writable == Some(false),
        });
    }
    Ok(VirtualDevice::register(
        snapshot.execution_target.clone(),
        snapshot.primary.clone(),
        volumes,
    )?)
}

async fn observe(path: &Path) -> FileState {
    let result = async {
        match tokio::fs::symlink_metadata(path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileState::Absent);
            }
            Err(error) => return Err(error),
            Ok(meta) if !meta.is_file() => return Err(std::io::Error::other("not a regular file")),
            Ok(_) => {}
        }
        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = vec![0; 1024 * 1024];
        let mut bytes = 0;
        let mut hash = Sha256::new();
        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            hash.update(&buffer[..n]);
        }
        Ok(FileState::File {
            bytes,
            sha256: hex::encode(hash.finalize()),
        })
    }
    .await;
    result.unwrap_or_else(|error: std::io::Error| FileState::Unobserved {
        reason: error.to_string(),
    })
}

fn snapshot_scope(
    plan: &UpdatePlan,
    authorization: &MapAuthorization,
) -> Result<BTreeMap<String, (SafeRelativePath, PlannedChange)>> {
    let mut paths = BTreeMap::new();
    for path in &plan.files_to_remove {
        paths.insert(
            path.to_string().to_ascii_lowercase(),
            (path.clone(), PlannedChange::Remove),
        );
    }
    for spec in &plan.downloads {
        paths.insert(
            spec.destination.to_string().to_ascii_lowercase(),
            (spec.destination.clone(), PlannedChange::Write),
        );
    }
    for unlock in &authorization.unlocks {
        let path = SafeRelativePath::parse(&unlock.file_name)?;
        paths.insert(
            path.to_string().to_ascii_lowercase(),
            (path, PlannedChange::Write),
        );
    }
    let manifest_path = SafeRelativePath::parse("Garmin/GarminDevice.xml")?;
    if paths.contains_key(&manifest_path.to_string().to_ascii_lowercase()) {
        bail!("an update cannot replace the device manifest");
    }
    paths.insert(
        manifest_path.to_string().to_ascii_lowercase(),
        (manifest_path, PlannedChange::Context),
    );
    Ok(paths)
}

const README: &str = indoc! {"
    # Simulated device transaction

    The physical source is read-only. `device/` holds virtual-device files;
    `before/` retains affected originals, grouped by storage key.
    This is an affected-files snapshot, not a complete device backup.

    - `snapshot.json`: scope and storage mapping.
    - `changes.json`: observed state, sizes, and hashes.
    - `summary.md`: readable report.

    Missing reports mean interruption or failed preparation, not success.
    Virtual capacity does not establish physical free space or firmware acceptance.

    This private capture may contain identifiers, maps, and authorization data.
    Do not publish it or replay its journals against a physical device.
"};
