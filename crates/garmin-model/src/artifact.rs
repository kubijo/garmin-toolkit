//! Immutable bytes and their acquisition and normalization provenance.

use std::{
    fmt,
    num::{NonZeroU32, NonZeroU64},
    str::FromStr,
    time::Duration,
};

use mediatype::MediaTypeBuf;
use thiserror::Error;

use crate::{
    identity::{Source, SourceId, UserId},
    value::{ComponentVersion, Timestamp, Transformation, trimmed_owned},
};

define_id!(
    ArtifactIdKind,
    ArtifactId,
    "artifact",
    "Type marker for immutable artifact IDs.",
    "An immutable artifact record ID."
);
define_id!(
    AcquisitionIdKind,
    AcquisitionId,
    "acquisition",
    "Type marker for acquisition IDs.",
    "One completed acquisition ID."
);
define_id!(
    AcquisitionOperationIdKind,
    AcquisitionOperationId,
    "acquisition-operation",
    "Type marker for acquisition-operation IDs.",
    "An ID shared by retries of one acquisition operation."
);
define_id!(
    NormalizationRunIdKind,
    NormalizationRunId,
    "normalization-run",
    "Type marker for normalization-run IDs.",
    "One versioned normalization attempt ID."
);

/// A validated media type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaType(MediaTypeBuf);

impl MediaType {
    #[must_use]
    pub const fn from_media_type(media_type: MediaTypeBuf) -> Self {
        Self(media_type)
    }

    #[must_use]
    pub const fn as_media_type(&self) -> &MediaTypeBuf {
        &self.0
    }

    #[must_use]
    pub fn into_media_type(self) -> MediaTypeBuf {
        self.0
    }
}

impl FromStr for MediaType {
    type Err = mediatype::MediaTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.canonicalize().fmt(formatter)
    }
}

/// A BLAKE3 digest of exact artifact bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ArtifactDigest(blake3::Hash);

impl serde::Serialize for ArtifactDigest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(self.0.as_bytes(), serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ArtifactDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = <[u8; 32] as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self(blake3::Hash::from_bytes(bytes)))
    }
}

impl ArtifactDigest {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes))
    }

    #[must_use]
    pub const fn from_blake3(hash: blake3::Hash) -> Self {
        Self(hash)
    }

    #[must_use]
    pub const fn as_blake3(&self) -> &blake3::Hash {
        &self.0
    }

    #[must_use]
    pub const fn into_blake3(self) -> blake3::Hash {
        self.0
    }
}

impl FromStr for ArtifactDigest {
    type Err = blake3::HexError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A non-negative byte count.
#[derive(Default)]
#[garmin_macros::portable(copy, ord, hash)]
pub struct ByteCount(u64);

impl ByteCount {
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn format_binary(self) -> String {
        let (value, unit) = self.format_binary_parts();
        format!("{value} {unit}")
    }

    #[must_use]
    pub fn format_binary_parts(self) -> (String, &'static str) {
        let (divisor, symbol) = if self >= Self::from_gibibytes(1) {
            (Self::from_gibibytes(1), "GiB")
        } else if self >= Self::from_mebibytes(1) {
            (Self::from_mebibytes(1), "MiB")
        } else if self >= Self::from_kibibytes(1) {
            (Self::from_kibibytes(1), "KiB")
        } else {
            return (self.0.to_string(), "B");
        };
        let divisor = u128::from(divisor.0);
        let tenths = (u128::from(self.0) * 10 + divisor / 2) / divisor;
        let whole = tenths / 10;
        let fraction = tenths % 10;
        (format!("{whole}.{fraction}"), symbol)
    }

    #[must_use]
    pub fn format_decimal(self, precision: usize) -> String {
        let scaled =
            byte_unit::Byte::from_u64(self.0).get_appropriate_unit(byte_unit::UnitType::Decimal);
        format!("{scaled:.precision$}")
    }

    #[must_use]
    pub fn ratio_scaled(self, total: Self, scale: u16) -> u16 {
        if total.0 == 0 {
            return 0;
        }
        let used = self.min(total);
        u16::try_from(u128::from(used.0) * u128::from(scale) / u128::from(total.0)).unwrap_or(scale)
    }

    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    #[must_use]
    pub const fn saturating_mul(self, multiplier: u32) -> Self {
        Self(self.0.saturating_mul(multiplier as u64))
    }

    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(bytes) => Some(Self(bytes)),
            None => None,
        }
    }

    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(bytes) => Some(Self(bytes)),
            None => None,
        }
    }

    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// Constructs a byte count from decimal megabytes (1,000,000 bytes each).
    /// # Panics
    /// Panics if the byte count exceeds `u64::MAX`.
    #[must_use]
    pub const fn from_megabytes(megabytes: u64) -> Self {
        Self(
            megabytes
                .checked_mul(1_000_000)
                .expect("megabyte count exceeds u64 bytes"),
        )
    }

    /// Constructs a byte count from decimal gigabytes (1,000,000,000 bytes each).
    /// # Panics
    /// Panics if the byte count exceeds `u64::MAX`.
    #[must_use]
    pub const fn from_gigabytes(gigabytes: u64) -> Self {
        Self(
            gigabytes
                .checked_mul(1_000_000_000)
                .expect("gigabyte count exceeds u64 bytes"),
        )
    }

    /// Constructs a byte count from binary kibibytes (1024 bytes each).
    /// # Panics
    /// Panics if the byte count exceeds `u64::MAX`.
    #[must_use]
    pub const fn from_kibibytes(kibibytes: u64) -> Self {
        Self(
            kibibytes
                .checked_mul(1024)
                .expect("kibibyte count exceeds u64 bytes"),
        )
    }

    /// Constructs a byte count from binary mebibytes (1024 kibibytes each).
    /// # Panics
    /// Panics if the byte count exceeds `u64::MAX`.
    #[must_use]
    pub const fn from_mebibytes(mebibytes: u64) -> Self {
        Self::from_kibibytes(
            mebibytes
                .checked_mul(1024)
                .expect("mebibyte count exceeds u64 bytes"),
        )
    }

    /// Constructs a byte count from binary gibibytes (1024 mebibytes each).
    /// # Panics
    /// Panics if the byte count exceeds `u64::MAX`.
    #[must_use]
    pub const fn from_gibibytes(gibibytes: u64) -> Self {
        Self(
            gibibytes
                .checked_mul(1 << 30)
                .expect("gibibyte count exceeds u64 bytes"),
        )
    }

    #[must_use]
    pub const fn as_bytes(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for ByteCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} B", self.0)
    }
}

/// A transfer rate in whole bytes per second.
#[garmin_macros::portable(copy, ord, hash)]
pub struct ByteRate(u64);

/// A transfer rate that can pace an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonZeroByteRate(NonZeroU64);

impl NonZeroByteRate {
    #[must_use]
    pub const fn try_from_bytes_per_second(rate: u64) -> Option<Self> {
        match NonZeroU64::new(rate) {
            Some(rate) => Some(Self(rate)),
            None => None,
        }
    }

    #[must_use]
    pub const fn from_bytes_per_second(rate: NonZeroU64) -> Self {
        Self(rate)
    }

    #[must_use]
    pub const fn as_bytes_per_second(self) -> NonZeroU64 {
        self.0
    }

    /// Time needed to transfer the given size, rounded up to a nanosecond.
    #[must_use]
    pub fn transfer_duration(self, size: ByteCount) -> Duration {
        let rate = self.0.get();
        let bytes = size.as_bytes();
        let seconds = bytes / rate;
        let nanos = (u128::from(bytes % rate) * 1_000_000_000).div_ceil(u128::from(rate));
        Duration::from_secs(seconds)
            + Duration::from_nanos(u64::try_from(nanos).unwrap_or(1_000_000_000))
    }
}

impl ByteRate {
    /// Computes the nearest whole-byte rate, saturating on overflow.
    #[must_use]
    pub fn from_transfer(bytes: ByteCount, elapsed: std::time::Duration) -> Self {
        if elapsed.is_zero() {
            return Self(0);
        }
        let nanoseconds = elapsed.as_nanos();
        let numerator = u128::from(bytes.as_bytes()) * 1_000_000_000;
        Self(u64::try_from((numerator + nanoseconds / 2) / nanoseconds).unwrap_or(u64::MAX))
    }

    #[must_use]
    pub const fn from_bytes_per_second(bytes_per_second: u64) -> Self {
        Self(bytes_per_second)
    }

    /// Rounds to whole bytes per second, saturating outside the nonnegative range.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the rounded rate saturates to the supported unsigned range"
    )]
    pub fn saturating_from_bytes_per_second_f64(bytes_per_second: f64) -> Self {
        Self(bytes_per_second.round() as u64)
    }

    #[must_use]
    pub const fn as_bytes_per_second(&self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn format_decimal(self, precision: usize) -> String {
        format!("{}/s", self.format_decimal_amount(precision))
    }

    #[must_use]
    pub fn format_decimal_amount(self, precision: usize) -> String {
        ByteCount::from_bytes(self.0).format_decimal(precision)
    }
}

/// An opaque identifier meaningful only within one connector source.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceIdentity(String);

impl SourceIdentity {
    /// Validates owned source identity text without interpreting it.
    /// # Errors
    /// [`Error::EmptySourceIdentity`] when the value is blank.
    pub fn from_string(value: String) -> Result<Self, Error> {
        if value.trim().is_empty() {
            return Err(Error::EmptySourceIdentity);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl FromStr for SourceIdentity {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_string(value.to_owned())
    }
}

impl fmt::Display for SourceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Metadata for one immutable stored artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Artifact {
    id: ArtifactId,
    digest: ArtifactDigest,
    media_type: MediaType,
    byte_count: ByteCount,
}

impl Artifact {
    #[must_use]
    pub fn from_bytes(id: ArtifactId, media_type: MediaType, bytes: &[u8]) -> Self {
        Self {
            id,
            digest: ArtifactDigest::from_bytes(bytes),
            media_type,
            byte_count: ByteCount::from_bytes(bytes.len() as u64),
        }
    }

    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    #[must_use]
    pub const fn digest(&self) -> ArtifactDigest {
        self.digest
    }

    #[must_use]
    pub const fn media_type(&self) -> &MediaType {
        &self.media_type
    }

    #[must_use]
    pub const fn byte_count(&self) -> ByteCount {
        self.byte_count
    }
}

/// Provenance for one completed read of an artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Acquisition {
    id: AcquisitionId,
    operation_id: AcquisitionOperationId,
    artifact_id: ArtifactId,
    owner_id: UserId,
    source_id: SourceId,
    source_identity: SourceIdentity,
    acquired_at: Timestamp,
}

impl Acquisition {
    #[must_use]
    pub const fn from_source(
        id: AcquisitionId,
        operation_id: AcquisitionOperationId,
        artifact_id: ArtifactId,
        source: &Source,
        source_identity: SourceIdentity,
        acquired_at: Timestamp,
    ) -> Self {
        Self {
            id,
            operation_id,
            artifact_id,
            owner_id: source.owner_id(),
            source_id: source.id(),
            source_identity,
            acquired_at,
        }
    }

    #[must_use]
    pub const fn id(&self) -> AcquisitionId {
        self.id
    }

    #[must_use]
    pub const fn operation_id(&self) -> AcquisitionOperationId {
        self.operation_id
    }

    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    #[must_use]
    pub const fn source_identity(&self) -> &SourceIdentity {
        &self.source_identity
    }

    #[must_use]
    pub const fn acquired_at(&self) -> Timestamp {
        self.acquired_at
    }
}

/// A non-zero normalized projection schema version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SchemaVersion(NonZeroU32);

impl SchemaVersion {
    /// Validates an integer schema version.
    /// # Errors
    /// [`Error::ZeroSchemaVersion`] for zero.
    pub const fn from_u32(version: u32) -> Result<Self, Error> {
        match NonZeroU32::new(version) {
            Some(version) => Ok(Self(version)),
            None => Err(Error::ZeroSchemaVersion),
        }
    }

    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        self.0.get()
    }

    #[must_use]
    pub const fn into_u32(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A stable failure summary retained for later reprocessing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizationFailure(String);

impl NormalizationFailure {
    /// Validates an owned failure summary.
    /// # Errors
    /// [`Error::EmptyFailure`] when the value is blank.
    pub fn from_string(value: String) -> Result<Self, Error> {
        let value = trimmed_owned(value);
        if value.is_empty() {
            return Err(Error::EmptyFailure);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for NormalizationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for NormalizationFailure {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_string(value.to_owned())
    }
}

/// Result of one normalization attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalizationOutcome {
    Succeeded,
    Failed(NormalizationFailure),
}

/// One reproducible normalization attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizationRun {
    id: NormalizationRunId,
    acquisition_id: AcquisitionId,
    artifact_id: ArtifactId,
    owner_id: UserId,
    parser: ComponentVersion,
    schema: SchemaVersion,
    transformations: Vec<Transformation>,
    outcome: NormalizationOutcome,
}

impl NormalizationRun {
    #[must_use]
    pub const fn from_parts(
        id: NormalizationRunId,
        acquisition: &Acquisition,
        parser: ComponentVersion,
        schema: SchemaVersion,
        transformations: Vec<Transformation>,
        outcome: NormalizationOutcome,
    ) -> Self {
        Self {
            id,
            acquisition_id: acquisition.id(),
            artifact_id: acquisition.artifact_id(),
            owner_id: acquisition.owner_id(),
            parser,
            schema,
            transformations,
            outcome,
        }
    }

    #[must_use]
    pub const fn id(&self) -> NormalizationRunId {
        self.id
    }

    #[must_use]
    pub const fn acquisition_id(&self) -> AcquisitionId {
        self.acquisition_id
    }

    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn parser(&self) -> &ComponentVersion {
        &self.parser
    }

    #[must_use]
    pub const fn schema(&self) -> SchemaVersion {
        self.schema
    }

    #[must_use]
    pub fn transformations(&self) -> &[Transformation] {
        &self.transformations
    }

    #[must_use]
    pub const fn outcome(&self) -> &NormalizationOutcome {
        &self.outcome
    }

    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(self.outcome, NormalizationOutcome::Succeeded)
    }
}

/// Invalid artifact-domain data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("source identity cannot be blank")]
    EmptySourceIdentity,
    #[error("normalization failure cannot be blank")]
    EmptyFailure,
    #[error("schema version cannot be zero")]
    ZeroSchemaVersion,
}
