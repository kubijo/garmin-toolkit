//! Versioned state in the visible `GARMIN-TOOLKIT` device namespace.

use base64::Engine as _;
use garmin_device::storage::{DeviceIoError, DeviceWrite};
use garmin_device::{DevicePathState, DevicePathStatus, SafeRelativePath};
use garmin_model::map::MapInstallIdentifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::str::FromStr as _;
use thiserror::Error;
use uuid::Uuid;

use crate::{BackupPolicy, DownloadSpec};

pub const DEVICE_STATE_MAGIC: &str = "garmin-toolkit-device-state";
pub const DEVICE_STATE_VERSION: u32 = 1;
const NAMESPACE: &str = "GARMIN-TOOLKIT";
const TRANSACTIONS: &str = "GARMIN-TOOLKIT/transactions";
const MANIFEST: &str = "GARMIN-TOOLKIT/manifest.toml";
const IDENTITY: &str = "GARMIN-TOOLKIT/identity.toml";
const ACTIVE: &str = "GARMIN-TOOLKIT/active.json";
const MANIFEST_LIMIT: u64 = 64 * 1024;
const IDENTITY_LIMIT: u64 = 64 * 1024;
const ACTIVE_LIMIT: u64 = 64 * 1024;
const PREPARED_LIMIT: u64 = 512 * 1024;
const EVENT_LIMIT: u64 = 8 * 1024;
const COMPLETION_LIMIT: u64 = 64 * 1024;
const NAMESPACE_LIMIT: u64 = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceStateHeader {
    magic: String,
    kind: String,
    version: u32,
}

impl DeviceStateHeader {
    fn current(kind: &str) -> Self {
        Self {
            magic: DEVICE_STATE_MAGIC.to_owned(),
            kind: kind.to_owned(),
            version: DEVICE_STATE_VERSION,
        }
    }

    fn validate(&self, kind: &str) -> Result<(), DeviceStateError> {
        if self.magic != DEVICE_STATE_MAGIC || self.kind != kind {
            return Err(DeviceStateError::InvalidHeader(kind.to_owned()));
        }
        if self.version != DEVICE_STATE_VERSION {
            return Err(DeviceStateError::UnsupportedVersion {
                kind: kind.to_owned(),
                version: self.version,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PortableDownloadIdentity {
    map_name: String,
    source: url::Url,
    alternate_sources: Vec<url::Url>,
    cache_name: String,
    destination: SafeRelativePath,
    size: u64,
    md5: String,
    sha256: String,
    requires_device_authorization: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PortableEmbeddedPayload {
    destination: SafeRelativePath,
    size: u64,
    sha256: String,
    base64: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTransactionKind {
    Update,
    Removal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviceTransactionOperationKind {
    Write,
    Remove,
}

pub(crate) struct DeviceOperationTarget {
    storage_id: String,
    storage_label: String,
    path: SafeRelativePath,
}

impl DeviceOperationTarget {
    pub(crate) fn new(storage_id: String, storage_label: String, path: SafeRelativePath) -> Self {
        Self {
            storage_id,
            storage_label,
            path,
        }
    }
}

pub(crate) struct DeviceContentIdentity {
    size: u64,
    sha256: String,
}

impl DeviceContentIdentity {
    pub(crate) fn new(size: u64, sha256: String) -> Self {
        Self { size, sha256 }
    }
}

pub(crate) enum DeviceOriginalState {
    Missing,
    Verified(DeviceContentIdentity),
    Unverified { size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeviceTransactionOperation {
    index: u32,
    kind: DeviceTransactionOperationKind,
    storage_id: String,
    storage_label: String,
    path: SafeRelativePath,
    expected_size: Option<u64>,
    expected_sha256: Option<String>,
    original_size: Option<u64>,
    original_sha256: Option<String>,
}

impl DeviceTransactionOperation {
    #[must_use]
    fn write(
        index: u32,
        target: DeviceOperationTarget,
        expected: DeviceContentIdentity,
        original: DeviceOriginalState,
    ) -> Self {
        let (original_size, original_sha256) = match original {
            DeviceOriginalState::Missing => (None, None),
            DeviceOriginalState::Verified(identity) => (Some(identity.size), Some(identity.sha256)),
            DeviceOriginalState::Unverified { size } => (Some(size), None),
        };
        Self {
            index,
            kind: DeviceTransactionOperationKind::Write,
            storage_id: target.storage_id,
            storage_label: target.storage_label,
            path: target.path,
            expected_size: Some(expected.size),
            expected_sha256: Some(expected.sha256),
            original_size,
            original_sha256,
        }
    }

    #[must_use]
    fn remove(index: u32, target: DeviceOperationTarget, original: DeviceContentIdentity) -> Self {
        Self {
            index,
            kind: DeviceTransactionOperationKind::Remove,
            storage_id: target.storage_id,
            storage_label: target.storage_label,
            path: target.path,
            expected_size: None,
            expected_sha256: None,
            original_size: Some(original.size),
            original_sha256: Some(original.sha256),
        }
    }

    fn remove_unverified(index: u32, target: DeviceOperationTarget, original_size: u64) -> Self {
        Self {
            index,
            kind: DeviceTransactionOperationKind::Remove,
            storage_id: target.storage_id,
            storage_label: target.storage_label,
            path: target.path,
            expected_size: None,
            expected_sha256: None,
            original_size: Some(original_size),
            original_sha256: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableTransaction(PortableTransactionV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PortableTransactionV1 {
    format: DeviceStateHeader,
    transaction_id: Uuid,
    kind: DeviceTransactionKind,
    device_digest: String,
    plan_digest: String,
    backup_policy: BackupPolicy,
    downloads: Vec<PortableDownloadIdentity>,
    embedded_payloads: Vec<PortableEmbeddedPayload>,
    identifiers: Vec<MapInstallIdentifier>,
    operations: Vec<DeviceTransactionOperation>,
}

impl PortableTransaction {
    #[must_use]
    pub(crate) fn new(
        kind: DeviceTransactionKind,
        device_digest: String,
        plan_digest: String,
        backup_policy: BackupPolicy,
    ) -> Self {
        Self(PortableTransactionV1 {
            format: DeviceStateHeader::current("prepared-transaction"),
            transaction_id: Uuid::new_v4(),
            kind,
            device_digest,
            plan_digest,
            backup_policy,
            downloads: Vec::new(),
            embedded_payloads: Vec::new(),
            identifiers: Vec::new(),
            operations: Vec::new(),
        })
    }

    pub(crate) fn add_download(&mut self, download: &DownloadSpec, sha256: String) {
        let mut source = download.source.clone();
        source.set_query(None);
        source.set_fragment(None);
        let alternate_sources = download
            .alternate_sources
            .iter()
            .cloned()
            .map(|mut source| {
                source.set_query(None);
                source.set_fragment(None);
                source
            })
            .collect();
        self.0.downloads.push(PortableDownloadIdentity {
            map_name: download.map_name.clone(),
            source,
            alternate_sources,
            cache_name: download.cache_name.clone(),
            destination: download.destination.clone(),
            size: download.size,
            md5: download.md5.clone(),
            sha256,
            requires_device_authorization: download.requires_garmin_token,
        });
    }

    pub(crate) fn set_identifiers(&mut self, identifiers: Vec<MapInstallIdentifier>) {
        self.0.identifiers = identifiers;
    }

    pub(crate) fn add_write(
        &mut self,
        target: DeviceOperationTarget,
        expected: DeviceContentIdentity,
        original: DeviceOriginalState,
    ) -> Result<(), DeviceStateError> {
        let index = self.next_operation_index()?;
        self.0.operations.push(DeviceTransactionOperation::write(
            index, target, expected, original,
        ));
        Ok(())
    }

    pub(crate) fn add_verified_removal(
        &mut self,
        target: DeviceOperationTarget,
        original: DeviceContentIdentity,
    ) -> Result<(), DeviceStateError> {
        let index = self.next_operation_index()?;
        self.0
            .operations
            .push(DeviceTransactionOperation::remove(index, target, original));
        Ok(())
    }

    pub(crate) fn add_unverified_removal(
        &mut self,
        target: DeviceOperationTarget,
        original_size: u64,
    ) -> Result<(), DeviceStateError> {
        let index = self.next_operation_index()?;
        self.0
            .operations
            .push(DeviceTransactionOperation::remove_unverified(
                index,
                target,
                original_size,
            ));
        Ok(())
    }

    #[must_use]
    pub const fn kind(&self) -> DeviceTransactionKind {
        self.0.kind
    }

    #[must_use]
    pub fn device_digest(&self) -> &str {
        &self.0.device_digest
    }

    #[must_use]
    pub fn plan_digest(&self) -> &str {
        &self.0.plan_digest
    }

    fn transaction_id(&self) -> Uuid {
        self.0.transaction_id
    }

    fn operations(&self) -> &[DeviceTransactionOperation] {
        &self.0.operations
    }

    fn next_operation_index(&self) -> Result<u32, DeviceStateError> {
        u32::try_from(self.0.operations.len()).map_err(|_| {
            DeviceStateError::InvalidTransaction("too many transaction operations".to_owned())
        })
    }

    fn encode(&self) -> Result<Vec<u8>, DeviceStateError> {
        let mut bytes = serde_json::to_vec_pretty(&self.0)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, DeviceStateError> {
        #[derive(Deserialize)]
        struct FormatProbe {
            format: DeviceStateHeader,
        }

        let probe: FormatProbe = serde_json::from_slice(bytes)?;
        probe.format.validate("prepared-transaction")?;
        let transaction = Self(serde_json::from_slice(bytes)?);
        transaction.validate()?;
        Ok(transaction)
    }

    fn validate(&self) -> Result<(), DeviceStateError> {
        self.0.format.validate("prepared-transaction")?;
        validate_device_digest(&self.0.device_digest)?;
        validate_sha256("plan", &self.0.plan_digest)?;
        self.validate_operations()?;
        self.validate_downloads()?;
        self.validate_embedded_payloads()
    }

    fn validate_operations(&self) -> Result<(), DeviceStateError> {
        if self.0.operations.is_empty() || self.0.operations.len() > MAX_EVENTS / 2 {
            return Err(DeviceStateError::InvalidTransaction(
                "operation count is outside the supported range".to_owned(),
            ));
        }
        let mut paths = BTreeSet::new();
        for (position, operation) in self.0.operations.iter().enumerate() {
            if usize::try_from(operation.index).ok() != Some(position) {
                return Err(DeviceStateError::InvalidTransaction(
                    "operation indexes are not contiguous".to_owned(),
                ));
            }
            if !paths.insert(operation.path.to_string().to_ascii_lowercase()) {
                return Err(DeviceStateError::InvalidTransaction(format!(
                    "duplicate target {}",
                    operation.path
                )));
            }
            if operation.storage_id.is_empty() || operation.storage_label.is_empty() {
                return Err(DeviceStateError::InvalidTransaction(format!(
                    "operation {} lacks a storage identity",
                    operation.path
                )));
            }
            match operation.kind {
                DeviceTransactionOperationKind::Write => {
                    if operation.expected_size.is_none()
                        || operation
                            .expected_sha256
                            .as_deref()
                            .is_none_or(|digest| !is_sha256(digest))
                    {
                        return Err(DeviceStateError::InvalidTransaction(format!(
                            "write {} lacks final content identity",
                            operation.path
                        )));
                    }
                }
                DeviceTransactionOperationKind::Remove => {
                    if operation.expected_size.is_some() || operation.expected_sha256.is_some() {
                        return Err(DeviceStateError::InvalidTransaction(format!(
                            "removal {} declares final bytes",
                            operation.path
                        )));
                    }
                }
            }
            if operation
                .original_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
            {
                return Err(DeviceStateError::InvalidTransaction(format!(
                    "operation {} has an invalid original digest",
                    operation.path
                )));
            }
        }
        Ok(())
    }

    fn validate_downloads(&self) -> Result<(), DeviceStateError> {
        for download in &self.0.downloads {
            if download.map_name.is_empty()
                || download.cache_name.is_empty()
                || download.size == 0
                || !is_md5(&download.md5)
                || !is_sha256(&download.sha256)
                || !portable_source(&download.source)
                || download
                    .alternate_sources
                    .iter()
                    .any(|source| !portable_source(source))
            {
                return Err(DeviceStateError::InvalidTransaction(format!(
                    "download identity for {} is incomplete",
                    download.destination
                )));
            }
        }
        Ok(())
    }

    fn validate_embedded_payloads(&self) -> Result<(), DeviceStateError> {
        let mut embedded_bytes = 0_u64;
        for payload in &self.0.embedded_payloads {
            if payload.size == 0 || !is_sha256(&payload.sha256) {
                return Err(DeviceStateError::InvalidTransaction(format!(
                    "embedded payload for {} is incomplete",
                    payload.destination
                )));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&payload.base64)
                .map_err(|_| {
                    DeviceStateError::InvalidTransaction(format!(
                        "embedded payload for {} is not base64",
                        payload.destination
                    ))
                })?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != payload.size
                || hex::encode(Sha256::digest(&bytes)) != payload.sha256
            {
                return Err(DeviceStateError::InvalidTransaction(format!(
                    "embedded payload for {} has the wrong identity",
                    payload.destination
                )));
            }
            embedded_bytes = embedded_bytes
                .checked_add(payload.size)
                .ok_or(DeviceStateError::NamespaceTooLarge)?;
        }
        if embedded_bytes > 1024 * 1024 {
            return Err(DeviceStateError::InvalidTransaction(
                "embedded recovery payloads exceed 1 MiB".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviceTransactionEventPhase {
    Started,
    Applied,
    Verified,
}

impl DeviceTransactionEventPhase {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Applied => "applied",
            Self::Verified => "verified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceTransactionEvent {
    format: DeviceStateHeader,
    transaction_id: Uuid,
    operation_index: u32,
    phase: DeviceTransactionEventPhase,
    path: SafeRelativePath,
}

impl DeviceTransactionEvent {
    fn new(
        transaction: &PortableTransaction,
        operation_index: u32,
        phase: DeviceTransactionEventPhase,
        path: SafeRelativePath,
    ) -> Self {
        Self {
            format: DeviceStateHeader::current("transaction-event"),
            transaction_id: transaction.transaction_id(),
            operation_index,
            phase,
            path,
        }
    }

    fn validate(&self, transaction: &PortableTransaction) -> Result<(), DeviceStateError> {
        self.format.validate("transaction-event")?;
        let operation = transaction
            .operations()
            .get(usize::try_from(self.operation_index).unwrap_or(usize::MAX))
            .ok_or_else(|| {
                DeviceStateError::InvalidTransaction("event operation is unknown".to_owned())
            })?;
        if self.transaction_id != transaction.transaction_id() || self.path != operation.path {
            return Err(DeviceStateError::InvalidTransaction(
                "event does not match its prepared transaction".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveTransaction {
    format: DeviceStateHeader,
    transaction_id: Uuid,
    device_digest: String,
    plan_digest: String,
}

impl ActiveTransaction {
    fn from_transaction(transaction: &PortableTransaction) -> Self {
        Self {
            format: DeviceStateHeader::current("active-transaction"),
            transaction_id: transaction.transaction_id(),
            device_digest: transaction.device_digest().to_owned(),
            plan_digest: transaction.plan_digest().to_owned(),
        }
    }

    fn validate(&self, device_digest: &str) -> Result<(), DeviceStateError> {
        self.format.validate("active-transaction")?;
        validate_device_digest(&self.device_digest)?;
        validate_sha256("plan", &self.plan_digest)?;
        if self.device_digest != device_digest {
            return Err(DeviceStateError::DeviceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CompletedTransaction {
    format: DeviceStateHeader,
    transaction_id: Uuid,
    device_digest: String,
    plan_digest: String,
}

impl CompletedTransaction {
    fn from_transaction(transaction: &PortableTransaction) -> Self {
        Self {
            format: DeviceStateHeader::current("completed-transaction"),
            transaction_id: transaction.transaction_id(),
            device_digest: transaction.device_digest().to_owned(),
            plan_digest: transaction.plan_digest().to_owned(),
        }
    }

    fn validate(&self, transaction: &PortableTransaction) -> Result<(), DeviceStateError> {
        self.format.validate("completed-transaction")?;
        if self.transaction_id != transaction.transaction_id()
            || self.device_digest != transaction.device_digest()
            || self.plan_digest != transaction.plan_digest()
        {
            return Err(DeviceStateError::InvalidTransaction(
                "completion marker does not match the transaction".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamespaceManifest {
    namespace_id: Uuid,
}

impl NamespaceManifest {
    fn new() -> Self {
        Self {
            namespace_id: Uuid::new_v4(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        format!(
            "[format]\nmagic = \"{DEVICE_STATE_MAGIC}\"\nkind = \"namespace\"\nversion = {DEVICE_STATE_VERSION}\n\n[namespace]\nid = \"{}\"\n",
            self.namespace_id
        )
        .into_bytes()
    }

    fn parse(bytes: &[u8]) -> Result<Self, DeviceStateError> {
        let text = std::str::from_utf8(bytes).map_err(|_| DeviceStateError::InvalidManifest)?;
        let document = toml_edit::DocumentMut::from_str(text)
            .map_err(|_| DeviceStateError::InvalidManifest)?;
        let magic = toml_string(&document, "format", "magic")?;
        let kind = toml_string(&document, "format", "kind")?;
        let version = document["format"]["version"]
            .as_integer()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(DeviceStateError::InvalidManifest)?;
        DeviceStateHeader {
            magic,
            kind,
            version,
        }
        .validate("namespace")?;
        let namespace_id = Uuid::parse_str(&toml_string(&document, "namespace", "id")?)
            .map_err(|_| DeviceStateError::InvalidManifest)?;
        Ok(Self { namespace_id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentityState(DeviceIdentityStateV1);

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceIdentityStateV1 {
    device_id: Uuid,
    device_digest: String,
    paired_user_id: Option<Uuid>,
}

impl DeviceIdentityState {
    #[must_use]
    pub fn new(device_digest: String, paired_user_id: Option<Uuid>) -> Self {
        Self(DeviceIdentityStateV1 {
            device_id: Uuid::new_v4(),
            device_digest,
            paired_user_id,
        })
    }

    #[must_use]
    pub const fn device_id(&self) -> Uuid {
        self.0.device_id
    }

    #[must_use]
    pub fn device_digest(&self) -> &str {
        &self.0.device_digest
    }

    #[must_use]
    pub const fn paired_user_id(&self) -> Option<Uuid> {
        self.0.paired_user_id
    }

    pub fn pair_with(&mut self, user_id: Uuid) {
        self.0.paired_user_id = Some(user_id);
    }

    pub fn clear_pairing(&mut self) {
        self.0.paired_user_id = None;
    }

    fn to_bytes(&self) -> Vec<u8> {
        let paired = self
            .0
            .paired_user_id
            .map_or_else(String::new, |id| format!("paired_user_id = \"{id}\"\n"));
        format!(
            "[format]\nmagic = \"{DEVICE_STATE_MAGIC}\"\nkind = \"identity\"\nversion = {DEVICE_STATE_VERSION}\n\n[identity]\ndevice_id = \"{}\"\ndevice_digest = \"{}\"\n{paired}",
            self.0.device_id, self.0.device_digest
        )
        .into_bytes()
    }

    fn parse(bytes: &[u8]) -> Result<Self, DeviceStateError> {
        let text = std::str::from_utf8(bytes).map_err(|_| DeviceStateError::InvalidIdentity)?;
        let document = toml_edit::DocumentMut::from_str(text)
            .map_err(|_| DeviceStateError::InvalidIdentity)?;
        let header = DeviceStateHeader {
            magic: toml_string(&document, "format", "magic")?,
            kind: toml_string(&document, "format", "kind")?,
            version: document["format"]["version"]
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(DeviceStateError::InvalidIdentity)?,
        };
        header.validate("identity")?;
        let device_digest = toml_string(&document, "identity", "device_digest")?;
        validate_device_digest(&device_digest)?;
        let device_id = Uuid::parse_str(&toml_string(&document, "identity", "device_id")?)
            .map_err(|_| DeviceStateError::InvalidIdentity)?;
        let paired_user_id = document["identity"]
            .get("paired_user_id")
            .and_then(toml_edit::Item::as_str)
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| DeviceStateError::InvalidIdentity)?;
        Ok(Self(DeviceIdentityStateV1 {
            device_id,
            device_digest,
            paired_user_id,
        }))
    }
}

pub struct DeviceTransactionStore<'a, D: DeviceWrite + ?Sized> {
    device: &'a D,
    storage: String,
}

impl<'a, D: DeviceWrite + ?Sized> DeviceTransactionStore<'a, D> {
    /// Open the device-state namespace.
    ///
    /// # Errors
    /// The primary storage must be identifiable.
    pub async fn open(device: &'a D) -> Result<Self, DeviceStateError> {
        Ok(Self {
            device,
            storage: device.primary_storage_id().await?,
        })
    }

    /// Read the active transaction.
    ///
    /// # Errors
    /// State must be readable, bounded, supported, and match the device.
    pub async fn active(
        &self,
        device_digest: &str,
    ) -> Result<Option<PortableTransaction>, DeviceStateError> {
        let Some(manifest) = self
            .read(SafeRelativePath::parse(MANIFEST)?, MANIFEST_LIMIT)
            .await?
        else {
            return Ok(None);
        };
        NamespaceManifest::parse(&manifest)?;
        let Some(active_bytes) = self
            .read(SafeRelativePath::parse(ACTIVE)?, ACTIVE_LIMIT)
            .await?
        else {
            return Ok(None);
        };
        let active: ActiveTransaction = serde_json::from_slice(&active_bytes)?;
        active.validate(device_digest)?;
        let prepared_path = prepared_path(active.transaction_id)?;
        let prepared_bytes = self
            .read(prepared_path, PREPARED_LIMIT)
            .await?
            .ok_or(DeviceStateError::MissingPrepared)?;
        let transaction = PortableTransaction::decode(&prepared_bytes)?;
        if transaction.transaction_id() != active.transaction_id
            || transaction.device_digest() != active.device_digest
            || transaction.plan_digest() != active.plan_digest
        {
            return Err(DeviceStateError::InvalidTransaction(
                "active marker does not match prepared transaction".to_owned(),
            ));
        }
        if let Some(completed) = self
            .read(
                completed_path(transaction.transaction_id())?,
                COMPLETION_LIMIT,
            )
            .await?
        {
            let completed: CompletedTransaction = serde_json::from_slice(&completed)?;
            completed.validate(&transaction)?;
            self.remove_active(&active_bytes).await?;
            self.cleanup(&transaction).await?;
            return Ok(None);
        }
        self.validate_namespace_size(&transaction).await?;
        Ok(Some(transaction))
    }

    pub(crate) async fn prepare(
        &self,
        transaction: &PortableTransaction,
    ) -> Result<(), DeviceStateError> {
        transaction.validate()?;
        self.ensure_namespace(transaction.device_digest()).await?;
        if self
            .read(SafeRelativePath::parse(ACTIVE)?, ACTIVE_LIMIT)
            .await?
            .is_some()
        {
            return Err(DeviceStateError::PendingTransaction);
        }
        self.device
            .ensure_directory(&self.storage, &SafeRelativePath::parse(TRANSACTIONS)?)
            .await?;
        self.device
            .ensure_directory(
                &self.storage,
                &transaction_directory(transaction.transaction_id())?,
            )
            .await?;
        self.device
            .ensure_directory(
                &self.storage,
                &events_directory(transaction.transaction_id())?,
            )
            .await?;
        let prepared_path = prepared_path(transaction.transaction_id())?;
        let bytes = transaction.encode()?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > PREPARED_LIMIT {
            return Err(DeviceStateError::DocumentTooLarge(
                prepared_path.to_string(),
            ));
        }
        self.create_or_validate(&prepared_path, &bytes).await?;
        self.create_json(
            &SafeRelativePath::parse(ACTIVE)?,
            &ActiveTransaction::from_transaction(transaction),
            ACTIVE_LIMIT,
        )
        .await
    }

    pub(crate) async fn event(
        &self,
        transaction: &PortableTransaction,
        operation_index: u32,
        phase: DeviceTransactionEventPhase,
    ) -> Result<(), DeviceStateError> {
        let operation = transaction
            .operations()
            .get(usize::try_from(operation_index).unwrap_or(usize::MAX))
            .ok_or_else(|| {
                DeviceStateError::InvalidTransaction("event operation is unknown".to_owned())
            })?;
        let event = DeviceTransactionEvent::new(
            transaction,
            operation_index,
            phase,
            operation.path.clone(),
        );
        event.validate(transaction)?;
        self.create_json(
            &event_path(transaction.transaction_id(), operation_index, phase)?,
            &event,
            EVENT_LIMIT,
        )
        .await
    }

    pub(crate) async fn finish(
        &self,
        transaction: &PortableTransaction,
    ) -> Result<(), DeviceStateError> {
        self.create_json(
            &completed_path(transaction.transaction_id())?,
            &CompletedTransaction::from_transaction(transaction),
            COMPLETION_LIMIT,
        )
        .await?;
        let active_path = SafeRelativePath::parse(ACTIVE)?;
        if let Some(active) = self.read(active_path.clone(), ACTIVE_LIMIT).await? {
            self.remove_file(&active_path, &active).await?;
        }
        self.cleanup(transaction).await
    }

    /// Read the device identity.
    ///
    /// # Errors
    /// The document must be readable and supported.
    pub async fn identity(&self) -> Result<Option<DeviceIdentityState>, DeviceStateError> {
        self.read(SafeRelativePath::parse(IDENTITY)?, IDENTITY_LIMIT)
            .await?
            .map(|bytes| DeviceIdentityState::parse(&bytes))
            .transpose()
    }

    /// Create the device identity.
    ///
    /// # Errors
    /// The identity must be valid and absent or identical.
    pub async fn create_identity(
        &self,
        identity: &DeviceIdentityState,
    ) -> Result<(), DeviceStateError> {
        validate_device_digest(identity.device_digest())?;
        self.ensure_namespace(identity.device_digest()).await?;
        let path = SafeRelativePath::parse(IDENTITY)?;
        let bytes = identity.to_bytes();
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > IDENTITY_LIMIT {
            return Err(DeviceStateError::DocumentTooLarge(IDENTITY.to_owned()));
        }
        self.create_or_validate(&path, &bytes).await
    }

    /// Clear a proven complete or untouched transaction.
    ///
    /// # Errors
    /// The transaction must be provably complete or untouched.
    pub async fn prove_and_clear(
        &self,
        transaction: &PortableTransaction,
    ) -> Result<(), DeviceStateError> {
        let mut final_state = true;
        let mut untouched = true;
        for operation in transaction.operations() {
            let status = self.device.inspect(&self.storage, &operation.path).await?;
            match operation.kind {
                DeviceTransactionOperationKind::Write => {
                    let Some(expected_size) = operation.expected_size else {
                        return Err(DeviceStateError::InvalidTransaction(
                            "write lacks expected size".to_owned(),
                        ));
                    };
                    final_state &= status
                        == (DevicePathStatus::RegularFile {
                            size: expected_size,
                        });
                    untouched &= match operation.original_size {
                        None => status == DevicePathStatus::Missing,
                        Some(size) => status == DevicePathStatus::RegularFile { size },
                    };
                }
                DeviceTransactionOperationKind::Remove => {
                    final_state &= status == DevicePathStatus::Missing;
                    untouched &= match operation.original_size {
                        Some(size) => status == DevicePathStatus::RegularFile { size },
                        None => false,
                    };
                }
            }
        }
        if !final_state && !untouched {
            return Err(DeviceStateError::CannotProveClear);
        }
        self.finish(transaction).await
    }

    async fn ensure_namespace(&self, device_digest: &str) -> Result<(), DeviceStateError> {
        self.device
            .ensure_directory(&self.storage, &SafeRelativePath::parse(NAMESPACE)?)
            .await?;
        let path = SafeRelativePath::parse(MANIFEST)?;
        if let Some(bytes) = self.read(path.clone(), MANIFEST_LIMIT).await? {
            NamespaceManifest::parse(&bytes)?;
        } else {
            let manifest = NamespaceManifest::new().to_bytes();
            self.create_or_validate(&path, &manifest).await?;
        }
        if let Some(identity) = self.identity().await?
            && identity.device_digest() != device_digest
        {
            return Err(DeviceStateError::DeviceMismatch);
        }
        Ok(())
    }

    async fn cleanup(&self, transaction: &PortableTransaction) -> Result<(), DeviceStateError> {
        let events = self
            .device
            .list_directory(
                &self.storage,
                &events_directory(transaction.transaction_id())?,
            )
            .await?;
        if events.len() > MAX_EVENTS {
            return Err(DeviceStateError::NamespaceTooLarge);
        }
        for entry in events {
            if entry.state != DevicePathState::RegularFile {
                return Err(DeviceStateError::UnexpectedNamespaceEntry(entry.name));
            }
            let path = SafeRelativePath::parse(format!(
                "{}/{}",
                events_directory(transaction.transaction_id())?,
                entry.name
            ))?;
            let bytes = self
                .read(path.clone(), EVENT_LIMIT)
                .await?
                .ok_or_else(|| DeviceStateError::UnexpectedNamespaceEntry(path.to_string()))?;
            let event: DeviceTransactionEvent = serde_json::from_slice(&bytes)?;
            event.validate(transaction)?;
            self.remove_file(&path, &bytes).await?;
        }
        self.device
            .remove_empty_directory(
                &self.storage,
                &events_directory(transaction.transaction_id())?,
            )
            .await?;
        for (path, limit) in [
            (prepared_path(transaction.transaction_id())?, PREPARED_LIMIT),
            (
                completed_path(transaction.transaction_id())?,
                COMPLETION_LIMIT,
            ),
        ] {
            if let Some(bytes) = self.read(path.clone(), limit).await? {
                self.remove_file(&path, &bytes).await?;
            }
        }
        self.device
            .remove_empty_directory(
                &self.storage,
                &transaction_directory(transaction.transaction_id())?,
            )
            .await?;
        Ok(())
    }

    async fn validate_namespace_size(
        &self,
        transaction: &PortableTransaction,
    ) -> Result<(), DeviceStateError> {
        let mut size = 0_u64;
        for (path, limit) in [
            (SafeRelativePath::parse(MANIFEST)?, MANIFEST_LIMIT),
            (SafeRelativePath::parse(ACTIVE)?, ACTIVE_LIMIT),
            (prepared_path(transaction.transaction_id())?, PREPARED_LIMIT),
        ] {
            if let DevicePathStatus::RegularFile { size: file_size } =
                self.device.inspect(&self.storage, &path).await?
            {
                if file_size > limit {
                    return Err(DeviceStateError::DocumentTooLarge(path.to_string()));
                }
                size = size.saturating_add(file_size);
            }
        }
        let events = self
            .device
            .list_directory(
                &self.storage,
                &events_directory(transaction.transaction_id())?,
            )
            .await?;
        if events.len() > MAX_EVENTS {
            return Err(DeviceStateError::NamespaceTooLarge);
        }
        for event in events {
            let Some(event_size) = event.size else {
                return Err(DeviceStateError::UnexpectedNamespaceEntry(event.name));
            };
            if event.state != DevicePathState::RegularFile || event_size > EVENT_LIMIT {
                return Err(DeviceStateError::UnexpectedNamespaceEntry(event.name));
            }
            size = size.saturating_add(event_size);
        }
        if size > NAMESPACE_LIMIT {
            return Err(DeviceStateError::NamespaceTooLarge);
        }
        Ok(())
    }

    async fn create_json<T: Serialize>(
        &self,
        path: &SafeRelativePath,
        value: &T,
        limit: u64,
    ) -> Result<(), DeviceStateError> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(DeviceStateError::DocumentTooLarge(path.to_string()));
        }
        self.create_or_validate(path, &bytes).await
    }

    async fn create_or_validate(
        &self,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceStateError> {
        match self
            .read(path.clone(), u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .await?
        {
            Some(observed) if observed == bytes => Ok(()),
            Some(_) => Err(DeviceStateError::ConflictingDocument(path.to_string())),
            None => {
                self.device
                    .create_verified_file(&self.storage, path, bytes)
                    .await?;
                let observed = self
                    .read(path.clone(), u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                    .await?
                    .ok_or_else(|| DeviceStateError::MissingDocument(path.to_string()))?;
                if observed != bytes {
                    return Err(DeviceStateError::ConflictingDocument(path.to_string()));
                }
                Ok(())
            }
        }
    }

    async fn read(
        &self,
        path: SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceStateError> {
        Ok(self
            .device
            .read_bounded_file(&self.storage, &path, limit)
            .await?)
    }

    async fn remove_active(&self, bytes: &[u8]) -> Result<(), DeviceStateError> {
        self.remove_file(&SafeRelativePath::parse(ACTIVE)?, bytes)
            .await
    }

    async fn remove_file(
        &self,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceStateError> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| DeviceStateError::DocumentTooLarge(path.to_string()))?;
        let sha256 = hex::encode(Sha256::digest(bytes));
        self.device
            .delete(&self.storage, path, size, &sha256)
            .await?;
        Ok(())
    }
}

fn transaction_directory(id: Uuid) -> Result<SafeRelativePath, DeviceStateError> {
    Ok(SafeRelativePath::parse(format!("{TRANSACTIONS}/{id}"))?)
}

fn events_directory(id: Uuid) -> Result<SafeRelativePath, DeviceStateError> {
    Ok(SafeRelativePath::parse(format!(
        "{TRANSACTIONS}/{id}/events"
    ))?)
}

fn prepared_path(id: Uuid) -> Result<SafeRelativePath, DeviceStateError> {
    Ok(SafeRelativePath::parse(format!(
        "{TRANSACTIONS}/{id}/prepared.json"
    ))?)
}

fn completed_path(id: Uuid) -> Result<SafeRelativePath, DeviceStateError> {
    Ok(SafeRelativePath::parse(format!(
        "{TRANSACTIONS}/{id}/completed.json"
    ))?)
}

fn event_path(
    id: Uuid,
    operation: u32,
    phase: DeviceTransactionEventPhase,
) -> Result<SafeRelativePath, DeviceStateError> {
    Ok(SafeRelativePath::parse(format!(
        "{TRANSACTIONS}/{id}/events/{:06}-{}.json",
        operation + 1,
        phase.suffix()
    ))?)
}

fn toml_string(
    document: &toml_edit::DocumentMut,
    table: &str,
    key: &str,
) -> Result<String, DeviceStateError> {
    document[table][key]
        .as_str()
        .map(str::to_owned)
        .ok_or(DeviceStateError::InvalidManifest)
}

fn validate_device_digest(digest: &str) -> Result<(), DeviceStateError> {
    if is_hex_digest(digest, 32) {
        Ok(())
    } else {
        Err(DeviceStateError::InvalidTransaction(
            "device digest is not a 32-character hexadecimal identity".to_owned(),
        ))
    }
}

fn validate_sha256(label: &str, digest: &str) -> Result<(), DeviceStateError> {
    if is_sha256(digest) {
        Ok(())
    } else {
        Err(DeviceStateError::InvalidTransaction(format!(
            "{label} digest is not SHA-256"
        )))
    }
}

fn is_sha256(value: &str) -> bool {
    is_hex_digest(value, 64)
}

fn is_md5(value: &str) -> bool {
    is_hex_digest(value, 32)
}

fn is_hex_digest(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn portable_source(source: &url::Url) -> bool {
    let trusted_transport = source.scheme() == "https"
        || (source.scheme() == "http"
            && match source.host() {
                Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                Some(url::Host::Ipv4(address)) => address.is_loopback(),
                Some(url::Host::Ipv6(address)) => address.is_loopback(),
                None => false,
            });
    trusted_transport && source.query().is_none() && source.fragment().is_none()
}

#[derive(Debug, Error)]
pub enum DeviceStateError {
    #[error(transparent)]
    Device(#[from] DeviceIoError),
    #[error(transparent)]
    Path(#[from] garmin_device::PathSafetyError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid {0} device-state header")]
    InvalidHeader(String),
    #[error("unsupported {kind} device-state version {version}")]
    UnsupportedVersion { kind: String, version: u32 },
    #[error("invalid GARMIN-TOOLKIT manifest")]
    InvalidManifest,
    #[error("invalid GARMIN-TOOLKIT identity")]
    InvalidIdentity,
    #[error("invalid portable transaction: {0}")]
    InvalidTransaction(String),
    #[error("portable transaction belongs to a different device")]
    DeviceMismatch,
    #[error("another unresolved device transaction already exists")]
    PendingTransaction,
    #[error("active transaction has no prepared document")]
    MissingPrepared,
    #[error("device-state document is missing after creation: {0}")]
    MissingDocument(String),
    #[error("device-state document conflicts with the expected immutable value: {0}")]
    ConflictingDocument(String),
    #[error("device-state document exceeds its configured limit: {0}")]
    DocumentTooLarge(String),
    #[error("GARMIN-TOOLKIT namespace exceeds 8 MiB")]
    NamespaceTooLarge,
    #[error("unexpected GARMIN-TOOLKIT entry: {0}")]
    UnexpectedNamespaceEntry(String),
    #[error("device state is neither fully committed nor provably untouched")]
    CannotProveClear,
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_device::storage::DirectoryDevice;
    use std::path::PathBuf;

    fn digest(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn transaction() -> PortableTransaction {
        let mut transaction = PortableTransaction::new(
            DeviceTransactionKind::Update,
            std::iter::repeat_n('a', 32).collect(),
            digest('b'),
            BackupPolicy::Skip,
        );
        transaction.add_download(
            &DownloadSpec {
                map_name: "Map".to_owned(),
                source: "https://download.example/map.img".parse().unwrap(),
                alternate_sources: Vec::new(),
                requires_garmin_token: false,
                destination: SafeRelativePath::parse("Garmin/map.img").unwrap(),
                cache_name: "map.img".to_owned(),
                size: 3,
                md5: std::iter::repeat_n('c', 32).collect(),
            },
            digest('d'),
        );
        transaction
            .add_write(
                DeviceOperationTarget::new(
                    "internal".to_owned(),
                    "Internal storage".to_owned(),
                    SafeRelativePath::parse("Garmin/map.img").unwrap(),
                ),
                DeviceContentIdentity::new(3, digest('d')),
                DeviceOriginalState::Missing,
            )
            .unwrap();
        transaction
    }

    #[tokio::test]
    async fn portable_state_survives_reopen_and_cleans_after_completion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("Garmin")).unwrap();
        let device = DirectoryDevice::new(PathBuf::from(root.path()));
        let store = DeviceTransactionStore::open(&device).await.unwrap();
        let transaction = transaction();

        store.prepare(&transaction).await.unwrap();
        assert_eq!(
            store.active(transaction.device_digest()).await.unwrap(),
            Some(transaction.clone())
        );
        store
            .event(&transaction, 0, DeviceTransactionEventPhase::Started)
            .await
            .unwrap();
        store.finish(&transaction).await.unwrap();

        assert!(
            store
                .active(transaction.device_digest())
                .await
                .unwrap()
                .is_none()
        );
        assert!(root.path().join(MANIFEST).is_file());
        assert!(!root.path().join(ACTIVE).exists());
    }

    #[tokio::test]
    async fn proven_clear_uses_path_and_size_without_reading_large_device_objects() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("Garmin")).unwrap();
        let device = DirectoryDevice::new(PathBuf::from(root.path()));
        let store = DeviceTransactionStore::open(&device).await.unwrap();
        let transaction = transaction();

        store.prepare(&transaction).await.unwrap();
        std::fs::write(root.path().join("Garmin/map.img"), b"bad").unwrap();
        store.prove_and_clear(&transaction).await.unwrap();

        assert!(
            store
                .active(transaction.device_digest())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read(root.path().join("Garmin/map.img")).unwrap(),
            b"bad",
            "map contents are intentionally left for the Garmin device to validate"
        );
    }

    #[test]
    fn every_json_document_has_a_versioned_header() {
        let transaction = transaction();
        let json: serde_json::Value =
            serde_json::from_slice(&transaction.encode().unwrap()).unwrap();
        assert_eq!(json["format"]["magic"], DEVICE_STATE_MAGIC);
        assert_eq!(json["format"]["version"], DEVICE_STATE_VERSION);
        assert_eq!(json["format"]["kind"], "prepared-transaction");
    }

    #[test]
    fn future_versions_fail_closed() {
        let transaction = transaction();
        let mut json: serde_json::Value =
            serde_json::from_slice(&transaction.encode().unwrap()).unwrap();
        json["format"]["version"] = serde_json::json!(DEVICE_STATE_VERSION + 1);
        let bytes = serde_json::to_vec(&json).unwrap();
        assert!(matches!(
            PortableTransaction::decode(&bytes),
            Err(DeviceStateError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn identity_state_round_trips_through_its_owned_boundary() {
        let user_id = Uuid::new_v4();
        let mut identity = DeviceIdentityState::new(std::iter::repeat_n('a', 32).collect(), None);
        identity.pair_with(user_id);

        let parsed = DeviceIdentityState::parse(&identity.to_bytes()).unwrap();
        assert_eq!(parsed.device_id(), identity.device_id());
        assert_eq!(parsed.device_digest(), identity.device_digest());
        assert_eq!(parsed.paired_user_id(), Some(user_id));

        identity.clear_pairing();
        assert_eq!(identity.paired_user_id(), None);
    }
}
