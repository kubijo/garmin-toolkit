use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const REGISTRY_VERSION: u8 = 1;
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const UPDATE_JOURNAL: &str = "mounted-update/transaction/000000-prepared.json";
const REMOVAL_JOURNAL: &str = "removal-transaction/000000-prepared.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PendingRecoveryKind {
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
pub(crate) struct PendingRecovery {
    receipt: PathBuf,
    pub(crate) kind: PendingRecoveryKind,
    pub(crate) transaction: PathBuf,
    device_digest: String,
    pub(crate) plan_digest: String,
    registered_unix_seconds: u64,
}

impl PendingRecovery {
    pub(crate) fn is_prepared(&self) -> Result<bool> {
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
pub(crate) struct PendingRecoveryStore {
    root: PathBuf,
}

impl PendingRecoveryStore {
    pub(crate) fn application() -> Result<Self> {
        let state = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("no user state directory is available for pending recoveries")?;
        Ok(Self::new(state.join("garmin-cli/pending-recoveries")))
    }

    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn register(
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

    pub(crate) fn register_capture(
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

    pub(crate) fn for_device(&self, device_digest: &str) -> Result<Vec<PendingRecovery>> {
        let mut recoveries = self
            .list()?
            .into_iter()
            .filter(|recovery| recovery.device_digest == device_digest)
            .collect::<Vec<_>>();
        recoveries.sort_by_key(|recovery| std::cmp::Reverse(recovery.registered_unix_seconds));
        Ok(recoveries)
    }

    pub(crate) fn for_selected_device(
        &self,
        device_digest: &str,
        active: Option<(PendingRecoveryKind, &str)>,
    ) -> Result<Option<PendingRecovery>> {
        let mut recoveries = self.for_device(device_digest)?.into_iter();
        Ok(match active {
            Some((kind, plan_digest)) => recoveries
                .find(|recovery| recovery.kind == kind && recovery.plan_digest == plan_digest),
            None => recoveries.next(),
        })
    }

    pub(crate) fn clear(&self, recovery: &PendingRecovery) -> Result<()> {
        if recovery.receipt.parent() != Some(self.root.as_path()) {
            bail!("pending-recovery receipt is outside its registry");
        }
        match fs::remove_file(&recovery.receipt) {
            Ok(()) => sync_directory(&self.root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "cannot clear pending-recovery receipt {}",
                    recovery.receipt.display()
                )
            }),
        }
    }

    pub(crate) fn discard_unprepared(&self, recovery: &PendingRecovery) -> Result<()> {
        if recovery.is_prepared()? {
            bail!("the device transaction was prepared; recover or prove its state")
        }
        self.clear(recovery)
    }

    fn list(&self) -> Result<Vec<PendingRecovery>> {
        let entries = match fs::read_dir(&self.root) {
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

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
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
}
