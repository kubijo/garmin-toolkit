//! Local Data Packages with application-independent metadata, archives, and table validation.

use std::{
    collections::BTreeSet,
    io::{Read, Seek},
    path::Path,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

mod archive;
pub mod descriptor;
mod validation;

pub use archive::Writer;
pub use descriptor::{
    Constraints, Descriptor, Field, FieldNames, ForeignKey, Metadata, Reference,
    ResourceDescriptor, TableSchema, V1, V2,
};
pub use validation::{Issue, IssueCode, Report};

/// Local package reading or writing failure, distinct from a table validation report.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid data package: {0}")]
    Invalid(String),
    #[error("unsupported data package feature: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Csv(#[from] csv::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

/// Limits applied to local files and decompressed ZIP members.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: u64,
    pub members: usize,
    pub descriptor_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 1024 * 1024 * 1024,
            members: 4096,
            descriptor_bytes: 4 * 1024 * 1024,
        }
    }
}

/// One resource and its local data,
/// with no application-specific interpretation.
#[derive(Clone, Debug)]
pub struct Resource {
    pub descriptor: ResourceDescriptor,
    pub bytes: Vec<u8>,
}

impl Resource {
    /// Deserializes a CSV relation into caller-owned Rust types.
    /// # Errors
    /// [`enum@Error`] for invalid CSV or a row incompatible with `T`.
    pub fn deserialize<T: DeserializeOwned>(&self) -> Result<Vec<T>, Error> {
        Ok(csv::Reader::from_reader(self.bytes.as_slice())
            .deserialize()
            .collect::<Result<_, _>>()?)
    }
}

/// A loaded local package.
/// Parsing and validation are separate so invalid fixtures remain inspectable.
#[derive(Debug)]
pub struct Package {
    pub descriptor: Descriptor,
    pub resources: Vec<Resource>,
    pub unlisted_members: Vec<String>,
}

impl Package {
    /// Reads an archive without extracting members or fetching remote resources.
    /// # Errors
    /// [`enum@Error`] for malformed metadata, unsafe paths, unsupported resources, or exceeded limits.
    pub fn read_zip(source: impl Read + Seek, limits: Limits) -> Result<Self, Error> {
        archive::read(source, limits)
    }

    /// Reads a descriptor and its listed local resources from one directory tree.
    /// # Errors
    /// [`enum@Error`] for malformed metadata, unsafe paths, unsupported resources, or exceeded limits.
    pub fn read_directory(descriptor: &Path, limits: Limits) -> Result<Self, Error> {
        let root = descriptor
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()?;
        let bytes = read_limited(std::fs::File::open(descriptor)?, limits.descriptor_bytes)?;
        Self::resolve(&bytes, limits, |path| {
            let path = root.join(path).canonicalize()?;
            ensure(
                path.starts_with(&root),
                "resource escapes package directory",
            )?;
            read_limited(std::fs::File::open(path)?, limits.bytes)
        })
    }

    /// Loads a descriptor through a caller-supplied local resource resolver.
    /// # Errors
    /// [`enum@Error`] for malformed metadata, unsafe paths, unsupported resources, or exceeded limits.
    pub fn resolve(
        descriptor: &[u8],
        limits: Limits,
        mut resolve: impl FnMut(&str) -> Result<Vec<u8>, Error>,
    ) -> Result<Self, Error> {
        ensure(
            descriptor.len() as u64 <= limits.descriptor_bytes,
            "descriptor exceeds size limit",
        )?;
        let descriptor: Descriptor = serde_json::from_slice(descriptor)?;
        ensure(
            descriptor
                .schema
                .as_deref()
                .is_none_or(|schema| schema == V1 || schema == V2),
            "unsupported descriptor profile",
        )?;
        ensure(
            descriptor.resources.len() <= limits.members,
            "too many resources",
        )?;
        let mut names = BTreeSet::new();
        let mut resources = Vec::new();
        let mut total = 0_u64;
        for metadata in &descriptor.resources {
            ensure(
                !metadata.name.is_empty() && names.insert(&metadata.name),
                "empty or duplicate resource name",
            )?;
            let bytes = match (&metadata.path, &metadata.data) {
                (Some(path), None) => {
                    ensure(safe_path(path), "unsafe resource path")?;
                    resolve(path)?
                }
                (None, Some(data)) => inline_csv(data)?,
                _ => {
                    return Err(Error::Invalid(
                        "resource needs exactly one path or data property".to_owned(),
                    ));
                }
            };
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| Error::Invalid("resource size overflow".to_owned()))?;
            ensure(total <= limits.bytes, "package exceeds size limit")?;
            resources.push(Resource {
                descriptor: metadata.clone(),
                bytes,
            });
        }
        Ok(Self {
            descriptor,
            resources,
            unlisted_members: Vec::new(),
        })
    }

    /// Validates supported local CSV/inline tables, declared integrity, types, and relations.
    #[must_use]
    pub fn validate(&self) -> Report {
        validation::validate(self)
    }
}

/// Serializes rows with headers even when the table contains no rows.
/// # Errors
/// [`enum@Error`] for incompatible row shapes or serialization failures.
pub fn csv_bytes<T: Serialize>(rows: &[T], schema: &TableSchema) -> Result<Vec<u8>, Error> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    writer.write_record(schema.fields.iter().map(|field| &field.name))?;
    for row in rows {
        writer.serialize(row)?;
    }
    Ok(writer
        .into_inner()
        .map_err(csv::IntoInnerError::into_error)?)
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
pub(crate) fn ensure(valid: bool, message: &str) -> Result<(), Error> {
    if valid {
        Ok(())
    } else {
        Err(Error::Invalid(message.to_owned()))
    }
}

pub(crate) fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.starts_with('~'))
}

fn read_limited(mut source: impl Read, limit: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    source
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure(bytes.len() as u64 <= limit, "resource exceeds size limit")?;
    Ok(bytes)
}

fn inline_csv(data: &Value) -> Result<Vec<u8>, Error> {
    let rows = data
        .as_array()
        .ok_or_else(|| Error::Unsupported("inline data must be row arrays".to_owned()))?;
    let mut writer = csv::Writer::from_writer(Vec::new());
    for row in rows {
        let cells = row
            .as_array()
            .ok_or_else(|| Error::Unsupported("inline data must be row arrays".to_owned()))?;
        writer.write_record(cells.iter().map(|value| match value {
            Value::Null => String::new(),
            Value::String(value) => value.clone(),
            value => value.to_string(),
        }))?;
    }
    Ok(writer
        .into_inner()
        .map_err(csv::IntoInnerError::into_error)?)
}
