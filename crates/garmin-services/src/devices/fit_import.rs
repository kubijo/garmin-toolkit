//! Review-bound identities for a selected device FIT acquisition.

use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, SourceIdentity},
    identity::Source,
};
use uuid::Uuid;

const OPERATION_DOMAIN: &[u8] = b"garmin-toolkit/device-files/acquisition/v1";
/// A random host approval and a repeatable job ID for exact device FIT bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plan {
    id: Uuid,
    job: Uuid,
    operation: AcquisitionOperationId,
}

impl Plan {
    #[must_use]
    pub fn for_file(source: &Source, identity: &SourceIdentity, bytes: &[u8]) -> Self {
        let mut digest = blake3::Hasher::new();
        digest.update(OPERATION_DOMAIN);
        digest.update(source.id().to_string().as_bytes());
        digest.update(&(identity.as_str().len() as u64).to_le_bytes());
        digest.update(identity.as_str().as_bytes());
        digest.update(ArtifactDigest::from_bytes(bytes).as_blake3().as_bytes());
        let operation = AcquisitionOperationId::from_u128(
            Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes()).as_u128(),
        );
        let job = Uuid::from_u128(operation.as_u128());
        Self {
            id: Uuid::new_v4(),
            job,
            operation,
        }
    }

    #[must_use]
    pub const fn id(self) -> Uuid {
        self.id
    }

    #[must_use]
    pub const fn job(self) -> Uuid {
        self.job
    }

    #[must_use]
    pub const fn operation(self) -> AcquisitionOperationId {
        self.operation
    }
}
