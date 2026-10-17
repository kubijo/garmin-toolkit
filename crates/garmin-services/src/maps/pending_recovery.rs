use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const REGISTRY_VERSION: u8 = 1;
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const UPDATE_JOURNAL: &str = "mounted-update/transaction/000000-prepared.json";
const REMOVAL_JOURNAL: &str = "removal-transaction/000000-prepared.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingRecoveryKind {
    Update,
    Removal,
}

impl PendingRecoveryKind {
    const fn journal_path(self) -> &'static str {
        match self {
            Self::Update => UPDATE_JOURNAL,
            Self::Removal => REMOVAL_JOURNAL,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingRecovery {
    receipt: PathBuf,
    pub kind: PendingRecoveryKind,
    pub transaction: PathBuf,
    device_digest: String,
    pub plan_digest: String,
    registered_unix_seconds: u64,
}

impl PendingRecovery {
    /// Whether this receipt belongs to a retained virtual target instead of its source device.
    /// # Errors
    /// Unsafe, oversized, unsupported, or mismatched execution metadata.
    pub fn is_simulated(&self) -> Result<bool> {
        #[derive(Deserialize)]
        struct Target {
            version: u8,
            device: String,
            plan: String,
            simulation: bool,
        }
        let path = self.transaction.join("workflow-target.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() > MAX_RECEIPT_BYTES {
            bail!("unsafe workflow target evidence");
        }
        let target: Target = serde_json::from_reader(File::open(path)?.take(MAX_RECEIPT_BYTES))?;
        if target.version != 1
            || target.device != self.device_digest
            || target.plan != self.plan_digest
        {
            bail!("workflow target does not match its recovery receipt");
        }
        if target.simulation && self.kind != PendingRecoveryKind::Update {
            bail!("unsupported simulated recovery kind");
        }
        Ok(target.simulation)
    }

    /// Whether the original capture contains a prepared transaction journal.
    /// # Errors
    /// Journal metadata is unreadable or refers to an unsafe object.
    pub fn is_prepared(&self) -> Result<bool> {
        let journal = self.transaction.join(self.kind.journal_path());
        match fs::symlink_metadata(&journal) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(true),
            Ok(_) => bail!("unsafe recovery journal {}", journal.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("cannot inspect recovery journal {}", journal.display())),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryReceipt {
    version: u8,
    kind: PendingRecoveryKind,
    transaction: PathBuf,
    device_digest: String,
    plan_digest: String,
    registered_unix_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct JournalIdentity {
    device_digest: String,
    plan_digest: String,
}

#[derive(Debug, Clone)]
pub struct PendingRecoveryStore {
    root: PathBuf,
    additional_roots: Vec<PathBuf>,
}

impl PendingRecoveryStore {
    /// Clears a completed operation's notice without hiding its successful outcome.
    pub fn clear_completed(&self, recovery: &PendingRecovery) {
        if let Err(error) = self.clear(recovery) {
            tracing::warn!(%error, capture = %recovery.transaction.display(),
                "completed recovery notice could not be cleared");
        }
    }

    /// Open the legacy application registry so existing CLI receipts remain discoverable.
    /// # Errors
    /// The platform supplies no user state directory.
    pub fn application() -> Result<Self> {
        let state = crate::paths::host_state_directory()?;
        Ok(Self::new(state.join("garmin-cli/pending-recoveries")))
    }

    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            additional_roots: Vec::new(),
        }
    }

    /// Also discover existing receipts without copying their original host evidence.
    #[must_use]
    pub fn with_registry(mut self, registry: Self) -> Self {
        if registry.root != self.root && !self.additional_roots.contains(&registry.root) {
            self.additional_roots.push(registry.root);
        }
        self
    }

    /// Durably register an operation before preparation or mutation begins.
    /// # Errors
    /// Invalid identities, unsafe paths, conflicting evidence, or filesystem failure.
    pub fn register(
        &self,
        kind: PendingRecoveryKind,
        transaction: &Path,
        device_digest: &str,
        plan_digest: &str,
    ) -> Result<PendingRecovery> {
        validate_identity(device_digest, plan_digest)?;
        let transaction = fs::canonicalize(transaction).with_context(|| {
            format!("cannot resolve recovery capture {}", transaction.display())
        })?;
        self.create_root()?;
        if let Some(existing) = self.list()?.into_iter().find(|recovery| {
            recovery.kind == kind
                && recovery.transaction == transaction
                && recovery.device_digest == device_digest
                && recovery.plan_digest == plan_digest
        }) {
            return Ok(existing);
        }
        let registered_unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("system time is before the Unix epoch")?
            .as_secs();
        let receipt = RecoveryReceipt {
            version: REGISTRY_VERSION,
            kind,
            transaction,
            device_digest: device_digest.to_owned(),
            plan_digest: plan_digest.to_owned(),
            registered_unix_seconds,
        };
        self.write_receipt(&receipt)
    }

    /// Register the identities read from an existing capture's prepared journal.
    /// # Errors
    /// Missing, unsafe, or invalid journal, or receipt persistence failure.
    pub fn register_capture(
        &self,
        kind: PendingRecoveryKind,
        transaction: &Path,
    ) -> Result<PendingRecovery> {
        let identity = read_journal_identity(&transaction.join(kind.journal_path()))?;
        self.register(
            kind,
            transaction,
            &identity.device_digest,
            &identity.plan_digest,
        )
    }

    /// List this device's receipts, most recent first.
    /// # Errors
    /// Unsafe or unreadable registry entries.
    pub fn for_device(&self, device_digest: &str) -> Result<Vec<PendingRecovery>> {
        let mut recoveries = self
            .list()?
            .into_iter()
            .filter(|recovery| recovery.device_digest == device_digest)
            .collect::<Vec<_>>();
        recoveries.sort_by_key(|recovery| std::cmp::Reverse(recovery.registered_unix_seconds));
        Ok(recoveries)
    }

    /// Match the active device transaction, or the latest host-only preparation.
    /// # Errors
    /// Unsafe or unreadable registry entries.
    pub fn for_selected_device(
        &self,
        device_digest: &str,
        active: Option<(PendingRecoveryKind, &str)>,
    ) -> Result<Option<PendingRecovery>> {
        let mut recoveries = self.for_device(device_digest)?.into_iter();
        if let Some((kind, plan_digest)) = active {
            for recovery in recoveries {
                if recovery.kind == kind
                    && recovery.plan_digest == plan_digest
                    && !recovery.is_simulated()?
                {
                    return Ok(Some(recovery));
                }
            }
            Ok(None)
        } else {
            Ok(recoveries.next())
        }
    }

    /// Remove a receipt belonging to this registry after its outcome is established.
    /// # Errors
    /// Foreign receipt path or filesystem failure.
    pub fn clear(&self, recovery: &PendingRecovery) -> Result<()> {
        let parent = recovery
            .receipt
            .parent()
            .context("receipt has no registry")?;
        if parent != self.root && !self.additional_roots.iter().any(|root| root == parent) {
            bail!("pending-recovery receipt is outside its registry");
        }
        match fs::remove_file(&recovery.receipt) {
            Ok(()) => sync_directory(parent),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "cannot clear pending-recovery receipt {}",
                    recovery.receipt.display()
                )
            }),
        }
    }

    /// Remove a preparation notice only before its transaction journal was written.
    /// # Errors
    /// Prepared transaction, unsafe evidence, or receipt removal failure.
    pub fn discard_unprepared(&self, recovery: &PendingRecovery) -> Result<()> {
        if recovery.is_prepared()? {
            bail!("the device transaction was prepared; recover or prove its state")
        }
        self.clear(recovery)
    }

    fn list(&self) -> Result<Vec<PendingRecovery>> {
        let mut receipts = Self::list_root(&self.root)?;
        for root in &self.additional_roots {
            receipts.extend(Self::list_root(root)?);
        }
        Ok(receipts)
    }

    fn list_root(root: &Path) -> Result<Vec<PendingRecovery>> {
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("cannot read the pending-recovery registry"),
        };
        entries
            .filter_map(|entry| match entry {
                Ok(entry)
                    if entry
                        .path()
                        .extension()
                        .is_some_and(|value| value == "json") =>
                {
                    Some(Self::read_receipt(&entry.path()))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error.into())),
            })
            .collect()
    }

    fn create_root(&self) -> Result<()> {
        fs::create_dir_all(&self.root).context("cannot create the pending-recovery registry")?;
        if !fs::symlink_metadata(&self.root)?.file_type().is_dir() {
            bail!(
                "pending-recovery registry is not a directory: {}",
                self.root.display()
            );
        }
        set_private_directory_permissions(&self.root)?;
        Ok(())
    }

    fn write_receipt(&self, receipt: &RecoveryReceipt) -> Result<PendingRecovery> {
        let path = self
            .root
            .join(format!("{}.json", uuid::Uuid::new_v4().simple()));
        let mut temporary = tempfile::Builder::new()
            .prefix(".pending-recovery-")
            .tempfile_in(&self.root)
            .context("cannot create a temporary recovery receipt")?;
        set_private_file_permissions(temporary.path())?;
        let file = temporary.as_file_mut();
        serde_json::to_writer_pretty(&mut *file, receipt)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
        temporary
            .persist_noclobber(&path)
            .map_err(|error| error.error)
            .with_context(|| format!("cannot publish recovery receipt {}", path.display()))?;
        sync_directory(&self.root)?;
        Ok(stored_recovery(path, receipt))
    }

    fn read_receipt(path: &Path) -> Result<PendingRecovery> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("cannot inspect recovery receipt {}", path.display()))?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_RECEIPT_BYTES {
            bail!("unsafe pending-recovery receipt {}", path.display());
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or_default());
        File::open(path)?
            .take(MAX_RECEIPT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECEIPT_BYTES {
            bail!("pending-recovery receipt is too large: {}", path.display());
        }
        let receipt: RecoveryReceipt = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid pending-recovery receipt {}", path.display()))?;
        if receipt.version != REGISTRY_VERSION || !receipt.transaction.is_absolute() {
            bail!("invalid pending-recovery receipt {}", path.display());
        }
        validate_identity(&receipt.device_digest, &receipt.plan_digest)?;
        Ok(stored_recovery(path.to_owned(), &receipt))
    }
}

fn stored_recovery(path: PathBuf, receipt: &RecoveryReceipt) -> PendingRecovery {
    PendingRecovery {
        receipt: path,
        kind: receipt.kind,
        transaction: receipt.transaction.clone(),
        device_digest: receipt.device_digest.clone(),
        plan_digest: receipt.plan_digest.clone(),
        registered_unix_seconds: receipt.registered_unix_seconds,
    }
}

fn read_journal_identity(path: &Path) -> Result<JournalIdentity> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect recovery journal {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > 4 * 1024 * 1024 {
        bail!("unsafe recovery journal {}", path.display());
    }
    let identity: JournalIdentity = serde_json::from_reader(
        File::open(path).with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| format!("invalid recovery journal {}", path.display()))?;
    validate_identity(&identity.device_digest, &identity.plan_digest)?;
    Ok(identity)
}

fn validate_identity(device_digest: &str, plan_digest: &str) -> Result<()> {
    if device_digest.is_empty() || plan_digest.is_empty() {
        bail!("pending recovery has an empty device or plan identity");
    }
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PendingRecoveryKind, PendingRecoveryStore};

    #[test]
    fn registers_deduplicates_lists_and_clears_a_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir(&capture).unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));

        let first = store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();
        let duplicate = store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();

        assert_eq!(first.receipt, duplicate.receipt);
        assert_eq!(store.for_device("device").unwrap().len(), 1);
        store.clear(&first).unwrap();
        assert!(store.for_device("device").unwrap().is_empty());
        assert!(
            capture.is_dir(),
            "clearing a notice must retain its capture"
        );
    }

    #[test]
    fn imports_identity_from_the_durable_transaction_journal() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        let journal = capture.join("mounted-update/transaction");
        std::fs::create_dir_all(&journal).unwrap();
        std::fs::write(
            journal.join("000000-prepared.json"),
            r#"{"device_digest":"device","plan_digest":"plan"}"#,
        )
        .unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));

        let recovery = store
            .register_capture(PendingRecoveryKind::Update, &capture)
            .unwrap();

        assert_eq!(recovery.device_digest, "device");
        assert_eq!(recovery.plan_digest, "plan");
        assert!(
            recovery
                .transaction
                .join(PendingRecoveryKind::Update.journal_path())
                .is_file()
        );
    }

    #[test]
    fn retained_receipt_is_discoverable_without_an_on_device_transaction() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir(&capture).unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));
        let expected = store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();

        let detected = store
            .for_selected_device("device", None)
            .unwrap()
            .expect("host receipt should keep an interrupted device discoverable");

        assert_eq!(detected.receipt, expected.receipt);
        assert_eq!(detected.transaction, capture.canonicalize().unwrap());
        assert!(!detected.is_prepared().unwrap());
    }

    #[test]
    fn only_an_update_journal_marks_an_update_as_prepared() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir(&capture).unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));
        let recovery = store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();
        std::fs::create_dir_all(capture.join("removal-transaction")).unwrap();
        std::fs::write(
            capture.join("removal-transaction/000000-prepared.json"),
            b"{}",
        )
        .unwrap();

        assert!(!recovery.is_prepared().unwrap());
        std::fs::create_dir_all(capture.join("mounted-update/transaction")).unwrap();
        std::fs::write(
            capture.join("mounted-update/transaction/000000-prepared.json"),
            b"{}",
        )
        .unwrap();
        assert!(recovery.is_prepared().unwrap());
    }

    #[test]
    fn discarding_an_unprepared_receipt_keeps_the_capture() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir(&capture).unwrap();
        std::fs::write(capture.join("completed-removal.json"), b"{}").unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));
        let recovery = store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();

        store.discard_unprepared(&recovery).unwrap();

        assert!(store.for_device("device").unwrap().is_empty());
        assert!(capture.join("completed-removal.json").is_file());
    }

    #[test]
    fn a_prepared_receipt_cannot_be_discarded() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir_all(capture.join("mounted-update/transaction")).unwrap();
        std::fs::write(
            capture.join("mounted-update/transaction/000000-prepared.json"),
            b"{}",
        )
        .unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));
        let recovery = store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();

        assert!(store.discard_unprepared(&recovery).is_err());
        assert_eq!(store.for_device("device").unwrap().len(), 1);
    }

    #[test]
    fn active_device_transaction_requires_the_exact_retained_plan() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir(&capture).unwrap();
        let store = PendingRecoveryStore::new(temporary.path().join("registry"));
        store
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();

        assert!(
            store
                .for_selected_device(
                    "device",
                    Some((PendingRecoveryKind::Update, "different-plan")),
                )
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn discovers_and_clears_legacy_receipts_in_their_original_registry() {
        let temporary = tempfile::tempdir().unwrap();
        let capture = temporary.path().join("capture");
        std::fs::create_dir(&capture).unwrap();
        let legacy = PendingRecoveryStore::new(temporary.path().join("legacy"));
        legacy
            .register(PendingRecoveryKind::Update, &capture, "device", "plan")
            .unwrap();
        let current = PendingRecoveryStore::new(temporary.path().join("current"))
            .with_registry(legacy.clone());
        let receipt = current
            .for_selected_device("device", None)
            .unwrap()
            .unwrap();
        current.discard_unprepared(&receipt).unwrap();
        assert!(legacy.for_device("device").unwrap().is_empty());
        assert!(capture.is_dir());
        assert!(!temporary.path().join("current").exists());
    }
}
