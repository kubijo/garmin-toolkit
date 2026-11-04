//! ZIP transport without application metadata or filesystem extraction.

use std::{
    collections::BTreeSet,
    io::{Read, Seek, Write},
};

use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    Descriptor, Error, Limits, Package, ResourceDescriptor, ensure, hash, read_limited, safe_path,
};

/// A ZIP writer that owns resource metadata and computes resource integrity fields.
pub struct Writer<W: Write + Seek> {
    archive: ZipWriter<W>,
    resources: Vec<ResourceDescriptor>,
}

impl<W: Write + Seek> Writer<W> {
    #[must_use]
    pub fn new(output: W) -> Self {
        Self {
            archive: ZipWriter::new(output),
            resources: Vec::new(),
        }
    }

    /// Adds a local resource and records the exact bytes' SHA-256 and length.
    /// # Errors
    /// [`enum@Error`] for unsafe or duplicate resources or ZIP failures.
    pub fn add(&mut self, mut descriptor: ResourceDescriptor, bytes: &[u8]) -> Result<(), Error> {
        let path = descriptor
            .path
            .as_deref()
            .ok_or_else(|| Error::Invalid("ZIP resources need a local path".to_owned()))?;
        ensure(
            safe_path(path) && path != "datapackage.json" && descriptor.data.is_none(),
            "invalid resource path",
        )?;
        ensure(
            !descriptor.name.is_empty()
                && self.resources.iter().all(|resource| {
                    resource.name != descriptor.name && resource.path != descriptor.path
                }),
            "duplicate resource name or path",
        )?;
        self.archive
            .start_file(path, SimpleFileOptions::default())?;
        self.archive.write_all(bytes)?;
        descriptor.hash = Some(hash(bytes));
        descriptor.bytes = Some(bytes.len() as u64);
        self.resources.push(descriptor);
        Ok(())
    }

    /// Writes the descriptor with the resources actually added to this writer.
    /// # Errors
    /// [`enum@Error`] for invalid metadata, JSON serialization, or ZIP failures.
    pub fn finish(mut self, mut descriptor: Descriptor) -> Result<W, Error> {
        ensure(
            descriptor.resources.is_empty(),
            "writer owns the descriptor's resources",
        )?;
        descriptor.resources = self.resources;
        self.archive
            .start_file("datapackage.json", SimpleFileOptions::default())?;
        self.archive
            .write_all(&serde_json::to_vec_pretty(&descriptor)?)?;
        Ok(self.archive.finish()?)
    }
}

pub(crate) fn read(source: impl Read + Seek, limits: Limits) -> Result<Package, Error> {
    let mut archive = ZipArchive::new(source)?;
    ensure(archive.len() <= limits.members, "too many ZIP members")?;
    let mut members = BTreeSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = file.name()?;
        if file.is_dir() {
            ensure(
                safe_path(name.trim_end_matches('/')),
                "unsafe ZIP directory",
            )?;
            continue;
        }
        ensure(safe_path(&name) && !file.is_symlink(), "unsafe ZIP member")?;
        ensure(members.insert(name.into_owned()), "duplicate ZIP member")?;
        total = total
            .checked_add(file.size())
            .ok_or_else(|| Error::Invalid("archive size overflow".to_owned()))?;
        ensure(total <= limits.bytes, "archive exceeds size limit")?;
    }
    let bytes = read_limited(
        archive.by_name("datapackage.json")?,
        limits.descriptor_bytes,
    )?;
    let mut package = Package::resolve(&bytes, limits, |path| {
        read_limited(archive.by_name(path)?, limits.bytes)
    })?;
    members.remove("datapackage.json");
    for resource in &package.resources {
        if let Some(path) = &resource.descriptor.path {
            members.remove(path);
        }
    }
    package.unlisted_members = members.into_iter().collect();
    Ok(package)
}
