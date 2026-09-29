//! Bounded inspection without transaction recovery or cleanup.

use super::*;
use garmin_device::storage::DeviceRead;
use garmin_model::device::{
    IdentityInspection, InspectionFailure, InspectionFailureKind, InspectionSection,
    StorageInspection, TransactionInspection, TransactionKind,
};

/// Reads only known state documents on one volume. Each section fails independently.
pub async fn inspect<D: DeviceRead + ?Sized>(
    device: &D,
    storage: &str,
    expected_digest: Option<&str>,
) -> StorageInspection {
    let namespace = section(
        read(device, storage, MANIFEST, MANIFEST_LIMIT)
            .await
            .and_then(|bytes| {
                bytes
                    .map(|bytes| NamespaceManifest::parse(&bytes).map(|m| m.namespace_id))
                    .transpose()
            }),
    );
    let identity = section(
        read(device, storage, IDENTITY, IDENTITY_LIMIT)
            .await
            .and_then(|bytes| {
                bytes
                    .map(|bytes| {
                        let identity = DeviceIdentityState::parse(&bytes)?;
                        if expected_digest.is_some_and(|digest| digest != identity.device_digest())
                        {
                            return Err(DeviceStateError::DeviceMismatch);
                        }
                        Ok(IdentityInspection {
                            device_id: identity.device_id(),
                            device_digest: identity.device_digest().to_owned(),
                            paired_user_id: identity.paired_user_id(),
                            verified: expected_digest.is_some(),
                        })
                    })
                    .transpose()
            }),
    );
    let transaction = section(transaction(device, storage, expected_digest).await);
    StorageInspection {
        storage_id: storage.to_owned(),
        namespace,
        identity,
        transaction,
    }
}

async fn read<D: DeviceRead + ?Sized>(
    device: &D,
    storage: &str,
    path: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, DeviceStateError> {
    let bytes = device
        .read_bounded_file(storage, &SafeRelativePath::parse(path)?, limit)
        .await?;
    if bytes
        .as_ref()
        .is_some_and(|bytes| bytes.len() as u64 > limit)
    {
        return Err(DeviceStateError::DocumentTooLarge(path.to_owned()));
    }
    Ok(bytes)
}

async fn transaction<D: DeviceRead + ?Sized>(
    device: &D,
    storage: &str,
    expected: Option<&str>,
) -> Result<Option<TransactionInspection>, DeviceStateError> {
    let Some(bytes) = read(device, storage, ACTIVE, ACTIVE_LIMIT).await? else {
        return Ok(None);
    };
    let active: ActiveTransaction = serde_json::from_slice(&bytes)?;
    active.validate(expected.unwrap_or(&active.device_digest))?;
    let path = prepared_path(active.transaction_id)?;
    let bytes = read(
        device,
        storage,
        path.as_utf8_path().as_str(),
        PREPARED_LIMIT,
    )
    .await?
    .ok_or(DeviceStateError::MissingPrepared)?;
    let prepared = PortableTransaction::decode(&bytes)?;
    if prepared.transaction_id() != active.transaction_id
        || prepared.device_digest() != active.device_digest
        || prepared.plan_digest() != active.plan_digest
    {
        return Err(DeviceStateError::InvalidTransaction(
            "active marker does not match prepared transaction".to_owned(),
        ));
    }
    let path = completed_path(active.transaction_id)?;
    let completion = read(
        device,
        storage,
        path.as_utf8_path().as_str(),
        COMPLETION_LIMIT,
    )
    .await?;
    if let Some(bytes) = &completion {
        let completed: CompletedTransaction = serde_json::from_slice(bytes)?;
        completed.validate(&prepared)?;
    }
    Ok(Some(TransactionInspection {
        transaction_id: active.transaction_id,
        kind: match prepared.kind() {
            DeviceTransactionKind::Update => TransactionKind::Update,
            DeviceTransactionKind::Removal => TransactionKind::Removal,
        },
        completed: completion.is_some(),
        verified: expected.is_some(),
    }))
}

fn section<T>(result: Result<Option<T>, DeviceStateError>) -> InspectionSection<T> {
    match result {
        Ok(Some(value)) => InspectionSection::Available(value),
        Ok(None) => InspectionSection::Missing,
        Err(error) => InspectionSection::Unavailable(InspectionFailure {
            kind: match &error {
                DeviceStateError::UnsupportedVersion { .. } => {
                    InspectionFailureKind::UnsupportedVersion
                }
                DeviceStateError::DeviceMismatch => InspectionFailureKind::IdentityMismatch,
                DeviceStateError::DocumentTooLarge(_)
                | DeviceStateError::Device(DeviceIoError::LimitExceeded(_)) => {
                    InspectionFailureKind::TooLarge
                }
                DeviceStateError::Device(_) => InspectionFailureKind::Unreadable,
                DeviceStateError::MissingPrepared => InspectionFailureKind::Incomplete,
                _ => InspectionFailureKind::Malformed,
            },
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
