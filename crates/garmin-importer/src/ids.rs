//! Retry-stable record IDs.

use std::fmt::Display;

use garmin_model::artifact::AcquisitionOperationId;
use newtype_uuid::{GenericUuid, TypedUuid, TypedUuidKind};
use uuid::Uuid;

pub fn derived_id<K>(
    namespace: &str,
    operation: AcquisitionOperationId,
    record: impl Display,
) -> TypedUuid<K>
where
    K: TypedUuidKind,
{
    // Retain this UUID v5 domain; retries and existing databases depend on its IDs.
    let name = format!("https://github.com/kubijo/nimrag/{namespace}/{operation}/{record}");
    TypedUuid::from_untyped_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()))
}
