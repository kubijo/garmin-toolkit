use super::*;
use std::{collections::BTreeMap, sync::Mutex};

/// Deliberately has no `DeviceWrite` implementation.
#[derive(Default)]
struct Reader {
    files: BTreeMap<String, Vec<u8>>,
    reads: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl DeviceRead for Reader {
    fn execution_target(&self) -> Option<&str> {
        None
    }
    async fn state(&self) -> Result<garmin_device::DeviceStateSnapshot, DeviceIoError> {
        panic!("unexpected capacity query")
    }
    async fn inventory(
        &self,
        _: &[SafeRelativePath],
    ) -> Result<garmin_device::DeviceInventory, DeviceIoError> {
        panic!("unexpected inventory crawl")
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        panic!("volume must be explicit")
    }
    async fn inspect(
        &self,
        _: &str,
        _: &SafeRelativePath,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        panic!("unexpected probe")
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
        panic!("unexpected verify")
    }
    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        assert_eq!(storage, "card");
        self.reads.lock().unwrap().push(path.to_string());
        let bytes = self.files.get(&path.to_string()).cloned();
        if bytes
            .as_ref()
            .is_some_and(|bytes| bytes.len() as u64 > limit)
        {
            return Err(DeviceIoError::LimitExceeded(path.to_string()));
        }
        Ok(bytes)
    }
}

fn reader() -> Reader {
    Reader {
        files: BTreeMap::from([
            (MANIFEST.to_owned(), NamespaceManifest::new().to_bytes()),
            (
                IDENTITY.to_owned(),
                DeviceIdentityState::new("a".repeat(32), Some(Uuid::new_v4())).to_bytes(),
            ),
        ]),
        ..Reader::default()
    }
}

#[tokio::test]
async fn absent_state_reads_only_the_three_known_paths() {
    let reader = Reader::default();
    let result = inspect(&reader, "card", None).await;
    assert_eq!(result.namespace, InspectionSection::Missing);
    assert_eq!(result.identity, InspectionSection::Missing);
    assert_eq!(result.transaction, InspectionSection::Missing);
    assert_eq!(*reader.reads.lock().unwrap(), [MANIFEST, IDENTITY, ACTIVE]);
}

#[tokio::test]
async fn corrupt_namespace_does_not_hide_valid_identity() {
    let mut reader = reader();
    reader
        .files
        .insert(MANIFEST.to_owned(), b"invalid".to_vec());
    let result = inspect(&reader, "card", Some(&"a".repeat(32))).await;
    assert!(matches!(
        result.namespace,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::Malformed,
            ..
        })
    ));
    assert!(matches!(
        result.identity,
        InspectionSection::Available(IdentityInspection {
            verified: true,
            paired_user_id: Some(_),
            ..
        })
    ));
}

#[tokio::test]
async fn limits_versions_and_identity_mismatch_remain_distinct() {
    let mut reader = reader();
    reader.files.insert(
        MANIFEST.to_owned(),
        vec![0; usize::try_from(MANIFEST_LIMIT).unwrap() + 1],
    );
    let result = inspect(&reader, "card", Some(&"b".repeat(32))).await;
    assert!(matches!(
        result.namespace,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::TooLarge,
            ..
        })
    ));
    assert!(matches!(
        result.identity,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::IdentityMismatch,
            ..
        })
    ));
    let future = String::from_utf8(NamespaceManifest::new().to_bytes())
        .unwrap()
        .replace("version = 1", "version = 2");
    reader
        .files
        .insert(MANIFEST.to_owned(), future.into_bytes());
    let result = inspect(&reader, "card", None).await;
    assert!(matches!(
        result.namespace,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::UnsupportedVersion,
            ..
        })
    ));
    assert!(matches!(
        result.identity,
        InspectionSection::Available(IdentityInspection {
            verified: false,
            ..
        })
    ));
}

#[tokio::test]
async fn pending_and_completed_markers_are_reported_without_cleanup() {
    let mut reader = reader();
    let transaction = crate::device_state::tests::transaction();
    let active = ActiveTransaction::from_transaction(&transaction);
    reader
        .files
        .insert(ACTIVE.to_owned(), serde_json::to_vec(&active).unwrap());
    let missing = inspect(&reader, "card", Some(&"a".repeat(32))).await;
    assert!(matches!(
        missing.transaction,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::Incomplete,
            ..
        })
    ));
    reader.files.insert(
        prepared_path(transaction.transaction_id())
            .unwrap()
            .to_string(),
        transaction.encode().unwrap(),
    );
    let pending = inspect(&reader, "card", Some(&"a".repeat(32))).await;
    assert!(matches!(
        pending.transaction,
        InspectionSection::Available(TransactionInspection {
            completed: false,
            verified: true,
            ..
        })
    ));
    reader.files.insert(
        completed_path(transaction.transaction_id())
            .unwrap()
            .to_string(),
        serde_json::to_vec(&CompletedTransaction::from_transaction(&transaction)).unwrap(),
    );
    let before = reader.files.clone();
    let complete = inspect(&reader, "card", Some(&"a".repeat(32))).await;
    assert!(matches!(
        complete.transaction,
        InspectionSection::Available(TransactionInspection {
            completed: true,
            ..
        })
    ));
    assert_eq!(reader.files, before);
    assert!(
        reader
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|path| before.contains_key(path))
    );
    let other = crate::device_state::tests::transaction();
    reader.files.insert(
        completed_path(transaction.transaction_id())
            .unwrap()
            .to_string(),
        serde_json::to_vec(&CompletedTransaction::from_transaction(&other)).unwrap(),
    );
    let invalid = inspect(&reader, "card", Some(&"a".repeat(32))).await;
    assert!(matches!(
        invalid.transaction,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::Malformed,
            ..
        })
    ));
}

#[tokio::test]
async fn invalid_active_header_is_not_followed() {
    let mut reader = reader();
    let transaction = PortableTransaction::new(
        DeviceTransactionKind::Update,
        "a".repeat(32),
        "b".repeat(64),
        BackupPolicy::Skip,
    );
    let mut active = ActiveTransaction::from_transaction(&transaction);
    active.format.version = 99;
    reader
        .files
        .insert(ACTIVE.to_owned(), serde_json::to_vec(&active).unwrap());
    let result = inspect(&reader, "card", None).await;
    assert!(matches!(
        result.transaction,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::UnsupportedVersion,
            ..
        })
    ));
    assert_eq!(*reader.reads.lock().unwrap(), [MANIFEST, IDENTITY, ACTIVE]);
}

#[tokio::test]
async fn malformed_pairing_is_not_reported_as_unpaired() {
    let mut reader = reader();
    let mut document = toml_edit::DocumentMut::from_str(
        std::str::from_utf8(reader.files.get(IDENTITY).unwrap()).unwrap(),
    )
    .unwrap();
    document["identity"]["paired_user_id"] = toml_edit::value(123);
    reader
        .files
        .insert(IDENTITY.to_owned(), document.to_string().into_bytes());
    let result = inspect(&reader, "card", None).await;
    assert!(matches!(
        result.identity,
        InspectionSection::Unavailable(InspectionFailure {
            kind: InspectionFailureKind::Malformed,
            ..
        })
    ));
}

#[tokio::test]
async fn valid_toml_with_missing_fields_returns_errors_instead_of_panicking() {
    let mut reader = reader();
    let mut namespace = toml_edit::DocumentMut::from_str(
        std::str::from_utf8(reader.files.get(MANIFEST).unwrap()).unwrap(),
    )
    .unwrap();
    namespace["format"]
        .as_table_mut()
        .unwrap()
        .remove("version");
    for bytes in [
        Vec::new(),
        b"format = 1".to_vec(),
        namespace.to_string().into_bytes(),
    ] {
        reader.files.insert(MANIFEST.to_owned(), bytes.clone());
        reader.files.insert(IDENTITY.to_owned(), bytes);
        let result = inspect(&reader, "card", None).await;
        assert!(matches!(
            result.namespace,
            InspectionSection::Unavailable(_)
        ));
        assert!(matches!(result.identity, InspectionSection::Unavailable(_)));
        assert_eq!(result.transaction, InspectionSection::Missing);
    }
}
