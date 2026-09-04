//! Linux desktop mount discovery.

use std::{
    collections::HashSet,
    fmt,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::SystemTime,
};

use crate::capabilities::{
    DataType, Error as ManifestError, Manifest, TransferDirection, is_device_manifest, parse,
};
use gio::{
    File, FileQueryInfoFlags, FileType, Mount, VolumeMonitor,
    glib::SignalHandlerId,
    prelude::{
        FileEnumeratorExt, FileExt, InputStreamExtManual, MountExt, ObjectExt, VolumeMonitorExt,
    },
};
use serde::Serialize;
use thiserror::Error;

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const FILE_ATTRIBUTES: &str = "standard::name,standard::type,standard::size,time::modified";

/// A file-manager-visible MTP mount that can be checked after consent.
#[derive(Clone, Serialize)]
pub struct MountedMtpCandidate {
    pub mount_id: String,
    pub name: String,
    #[serde(skip)]
    root: File,
}

impl MountedMtpCandidate {
    #[must_use]
    pub fn attachment_key(&self) -> &str {
        &self.mount_id
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.name
    }

    pub(crate) fn root(&self) -> &File {
        &self.root
    }

    /// Discovers the shared manifest and readable files after consent.
    /// # Errors
    /// Inaccessible, invalid, or ambiguous device data.
    pub fn scan(&self) -> Result<Vec<Catalog>, Error> {
        let entries = entries(&self.root)?;
        let manifests = entries
            .iter()
            .filter_map(|entry| storage_prefix(&entry.relative).map(|prefix| (prefix, entry)))
            .collect::<Vec<_>>();
        if manifests.is_empty() {
            return Err(Error::ManifestMissing);
        }
        let mut prefixes = HashSet::new();
        manifests
            .into_iter()
            .map(|(prefix, manifest_entry)| {
                if !prefixes.insert(prefix.clone()) {
                    return Err(Error::AmbiguousManifest);
                }
                scan_storage(&entries, &prefix, manifest_entry)
            })
            .collect()
    }
}

impl PartialEq for MountedMtpCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.mount_id == other.mount_id && self.name == other.name
    }
}

impl Eq for MountedMtpCandidate {}

impl crate::attachments::Candidate for MountedMtpCandidate {
    fn key(&self) -> &str {
        self.attachment_key()
    }

    fn name(&self) -> &str {
        self.display_name()
    }

    fn inspect(&self) -> Result<crate::attachments::Metadata, String> {
        let catalogs = self.scan().map_err(|error| error.to_string())?;
        let first = catalogs
            .first()
            .ok_or_else(|| "the device exposed no manifest-bearing storage".to_owned())?;
        let manifest = first.manifest();
        let mut capabilities = Vec::new();
        for catalog in &catalogs {
            let current = catalog.manifest();
            if current.id() != manifest.id() || current.model() != manifest.model() {
                return Err("the device exposed conflicting manifests".to_owned());
            }
            for capability in current.capabilities() {
                let value = crate::attachments::Capability::new(
                    capability.data_type(),
                    capability.direction(),
                );
                if !capabilities.contains(&value) {
                    capabilities.push(value);
                }
            }
        }
        Ok(crate::attachments::Metadata {
            id: manifest.id(),
            name: normalize_display_name(manifest.model().description()),
            software_version: manifest.model().software_version(),
            capabilities,
            storage: super::mounted_mtp::mounted_device_state_blocking(&self.mount_id)
                .map_err(|error| error.to_string())?,
        })
    }
}

impl fmt::Debug for MountedMtpCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MountedMtpCandidate")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// GIO mount notifications plus explicit reconciliation snapshots.
///
/// Construct and poll this on the desktop main thread. [`Self::poll_changed`]
/// dispatches pending `GLib` work; the application should call [`Self::candidates`]
/// at startup, after a change, and on a periodic repair timer.
pub struct MountedMtpMonitor {
    inner: VolumeMonitor,
    changes: Receiver<()>,
    handlers: Option<[SignalHandlerId; 3]>,
}

impl MountedMtpMonitor {
    #[must_use]
    pub fn new() -> Self {
        let inner = VolumeMonitor::get();
        let (sender, changes) = channel();
        let added_sender = sender.clone();
        let added_handler = inner.connect_mount_added(move |_, _| {
            let _ignored = added_sender.send(());
        });
        let changed_sender = sender.clone();
        let changed_handler = inner.connect_mount_changed(move |_, _| {
            let _ignored = changed_sender.send(());
        });
        let removed_handler = inner.connect_mount_removed(move |_, _| {
            let _ignored = sender.send(());
        });
        Self {
            inner,
            changes,
            handlers: Some([added_handler, changed_handler, removed_handler]),
        }
    }

    #[must_use]
    pub fn candidates(&self) -> Vec<MountedMtpCandidate> {
        candidates(&self.inner)
    }

    #[must_use]
    pub fn poll_changed(&self) -> bool {
        let context = gio::glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }

        let mut changed = false;
        loop {
            match self.changes.try_recv() {
                Ok(()) => changed = true,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return changed,
            }
        }
    }
}

impl crate::attachments::Backend for MountedMtpMonitor {
    type Candidate = MountedMtpCandidate;

    fn poll_changed(&self) -> bool {
        self.poll_changed()
    }

    fn candidates(&self) -> Vec<Self::Candidate> {
        self.candidates()
    }
}

pub(crate) fn mounted_candidates() -> Vec<MountedMtpCandidate> {
    candidates(&VolumeMonitor::get())
}

fn candidates(inner: &VolumeMonitor) -> Vec<MountedMtpCandidate> {
    let mut candidates = Vec::<MountedMtpCandidate>::new();
    for candidate in inner
        .mounts()
        .into_iter()
        .filter_map(|mount| candidate(&mount))
    {
        if let Some(existing) = candidates
            .iter_mut()
            .find(|existing| existing.mount_id == candidate.mount_id)
        {
            if mount_name_score(&candidate.name) > mount_name_score(&existing.name) {
                *existing = candidate;
            }
        } else {
            candidates.push(candidate);
        }
    }
    candidates.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.mount_id.cmp(&right.mount_id))
    });
    candidates
}

impl Default for MountedMtpMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MountedMtpMonitor {
    fn drop(&mut self) {
        if let Some(handlers) = self.handlers.take() {
            for handler in handlers {
                self.inner.disconnect(handler);
            }
        }
    }
}

impl fmt::Debug for MountedMtpMonitor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MountedMtpMonitor")
            .finish_non_exhaustive()
    }
}

fn candidate(mount: &Mount) -> Option<MountedMtpCandidate> {
    let root = mount.root();
    if root.uri_scheme().as_deref() != Some("mtp") {
        return None;
    }
    let root_name = root
        .query_info(
            "standard::display-name",
            FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()
        .map(|info| info.display_name().to_string());
    Some(MountedMtpCandidate {
        mount_id: crate::system::mounted_mtp::mount_id_for_uri(&root.uri()),
        name: preferred_name(root_name.as_deref(), mount.name().as_str()),
        root,
    })
}

fn preferred_name(root_name: Option<&str>, mount_name: &str) -> String {
    normalize_display_name(
        root_name
            .filter(|name| !name.is_empty())
            .unwrap_or(mount_name),
    )
}

pub(crate) fn normalize_display_name(name: &str) -> String {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first.to_uppercase().chain(characters).collect()
}

fn mount_name_score(name: &str) -> u8 {
    u8::from(!name.is_empty()) + u8::from(!name.eq_ignore_ascii_case("mtp"))
}

/// A validated manifest and its readable GIO files.
pub struct Catalog {
    manifest: Manifest,
    files: Vec<DeviceFile>,
}

impl Catalog {
    #[must_use]
    pub const fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    #[must_use]
    pub fn files(&self) -> &[DeviceFile] {
        &self.files
    }
}

/// One readable file selected through a validated manifest capability.
pub struct DeviceFile {
    source: File,
    relative: PathBuf,
    data_type: DataType,
    direction: TransferDirection,
    declared_size: u64,
    modified: SystemTime,
}

impl DeviceFile {
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        self.data_type
    }

    #[must_use]
    pub const fn direction(&self) -> TransferDirection {
        self.direction
    }

    #[must_use]
    pub const fn declared_size(&self) -> u64 {
        self.declared_size
    }

    /// Streams this complete file into caller-owned staging.
    ///
    /// Any error may leave an unpublished prefix in the target; discard it.
    /// # Errors
    /// Source, target, or length failures.
    pub fn copy_to<W>(&self, target: &mut W) -> Result<CopyOutcome, Error>
    where
        W: Write + ?Sized,
    {
        let source = self.source.read(gio::Cancellable::NONE)?;
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut received = 0_u64;
        loop {
            let count = source.read(&mut buffer, gio::Cancellable::NONE)?;
            if count == 0 {
                break;
            }
            received = received
                .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
                .ok_or(Error::SizeMismatch {
                    declared: self.declared_size,
                    received: u64::MAX,
                })?;
            if received > self.declared_size {
                return Err(Error::SizeMismatch {
                    declared: self.declared_size,
                    received,
                });
            }
            target.write_all(&buffer[..count]).map_err(Error::Target)?;
        }
        if received != self.declared_size {
            return Err(Error::SizeMismatch {
                declared: self.declared_size,
                received,
            });
        }
        Ok(CopyOutcome { bytes: received })
    }
}

impl fmt::Debug for DeviceFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceFile")
            .field("data_type", &self.data_type)
            .field("direction", &self.direction)
            .field("declared_size", &self.declared_size)
            .finish_non_exhaustive()
    }
}

/// A completed mount-to-staging copy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyOutcome {
    bytes: u64,
}

impl CopyOutcome {
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

/// A GIO-mounted device read failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("mounted device operation failed: {0}")]
    Gio(#[from] gio::glib::Error),
    #[error("mounted device contained an unsafe filename component")]
    UnsafeObjectName,
    #[error("mounted device reported a negative file size")]
    NegativeSize,
    #[error("GarminDevice.xml was not found")]
    ManifestMissing,
    #[error("multiple GarminDevice.xml files occupied the canonical path")]
    AmbiguousManifest,
    #[error("GarminDevice.xml exceeded {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge,
    #[error("GarminDevice.xml was not UTF-8: {0}")]
    ManifestUtf8(#[from] std::str::Utf8Error),
    #[error("GarminDevice.xml was rejected: {0}")]
    Manifest(#[from] ManifestError),
    #[error("staging write failed: {0}")]
    Target(std::io::Error),
    #[error("mounted source size mismatch: declared {declared}, received {received}")]
    SizeMismatch {
        /// Discovery size.
        declared: u64,
        /// Transferred size.
        received: u64,
    },
}

struct Entry {
    source: File,
    relative: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn entries(root: &File) -> Result<Vec<Entry>, Error> {
    let mut directories = vec![(root.clone(), PathBuf::new())];
    let mut entries = Vec::new();
    while let Some((directory, parent)) = directories.pop() {
        let enumerator = directory.enumerate_children(
            FILE_ATTRIBUTES,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )?;
        while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
            let name = info.name();
            validate_component(&name)?;
            let relative = parent.join(&name);
            let source = enumerator.child(&info);
            match info.file_type() {
                FileType::Directory => directories.push((source, relative)),
                FileType::Regular => entries.push(Entry {
                    source,
                    relative,
                    size: u64::try_from(info.size()).map_err(|_| Error::NegativeSize)?,
                    modified: info.modification_time(),
                }),
                _ => {}
            }
        }
    }
    Ok(entries)
}

fn validate_component(name: &Path) -> Result<(), Error> {
    let mut components = name.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(Error::UnsafeObjectName);
    }
    Ok(())
}

fn read_manifest(source: &File) -> Result<Vec<u8>, Error> {
    let stream = source.read(gio::Cancellable::NONE)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = stream.read(&mut buffer, gio::Cancellable::NONE)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
            return Err(Error::ManifestTooLarge);
        }
    }
    Ok(bytes)
}

fn storage_prefix(path: &Path) -> Option<PathBuf> {
    let components = path.components().collect::<Vec<_>>();
    (0..components.len()).find_map(|index| {
        let suffix = components[index..].iter().collect::<PathBuf>();
        is_device_manifest(&suffix).then(|| components[..index].iter().collect::<PathBuf>())
    })
}

fn scan_storage(
    entries: &[Entry],
    prefix: &Path,
    manifest_entry: &Entry,
) -> Result<Catalog, Error> {
    let manifest = read_manifest(&manifest_entry.source)?;
    let manifest = parse(std::str::from_utf8(&manifest)?)?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for capability in manifest.capabilities().iter().filter(|capability| {
        matches!(
            capability.direction(),
            TransferDirection::OutputFromUnit | TransferDirection::InputOutput
        )
    }) {
        for entry in entries {
            let Ok(relative) = entry.relative.strip_prefix(prefix) else {
                continue;
            };
            if capability.accepts(relative)
                && seen.insert((relative.to_path_buf(), capability.data_type()))
            {
                files.push(DeviceFile {
                    source: entry.source.clone(),
                    relative: relative.to_path_buf(),
                    data_type: capability.data_type(),
                    direction: capability.direction(),
                    declared_size: entry.size,
                    modified: entry.modified,
                });
            }
        }
    }
    files.sort_by(file_order);
    Ok(Catalog { manifest, files })
}

fn file_order(left: &DeviceFile, right: &DeviceFile) -> std::cmp::Ordering {
    right
        .modified
        .cmp(&left.modified)
        .then_with(|| left.relative.cmp(&right.relative))
}

#[cfg(test)]
mod tests {
    use std::{error::Error as StdError, fs, io, io::Write, path::Path};

    use gio::File;
    use serde::Serialize;
    use tempfile::tempdir;

    use super::{
        Error, MountedMtpCandidate, normalize_display_name, preferred_name, read_manifest,
        storage_prefix, validate_component,
    };

    const NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";

    #[test]
    fn root_display_name_overrides_the_mount_label() {
        assert_eq!(
            preferred_name(Some("fenix 8 - 47mm"), "091e 51b4"),
            "Fenix 8 - 47mm"
        );
        assert_eq!(preferred_name(None, "Edge 1050"), "Edge 1050");
        assert_eq!(normalize_display_name("fēnix 8"), "Fēnix 8");
    }

    #[derive(Serialize)]
    #[serde(rename = "Device")]
    struct ManifestFixture {
        #[serde(rename = "@xmlns")]
        namespace: &'static str,
        #[serde(rename = "Model")]
        model: ModelFixture,
        #[serde(rename = "Id")]
        id: u32,
        #[serde(rename = "MassStorageMode")]
        mass_storage: StorageFixture,
    }

    #[derive(Serialize)]
    struct ModelFixture {
        #[serde(rename = "SoftwareVersion")]
        version: u32,
        #[serde(rename = "Description")]
        description: &'static str,
    }

    #[derive(Serialize)]
    struct StorageFixture {
        #[serde(rename = "DataType")]
        data_type: DataTypeFixture,
    }

    #[derive(Serialize)]
    struct DataTypeFixture {
        #[serde(rename = "Name")]
        name: &'static str,
        #[serde(rename = "File")]
        file: FileFixture,
    }

    #[derive(Serialize)]
    struct FileFixture {
        #[serde(rename = "Specification")]
        specification: SpecificationFixture,
        #[serde(rename = "Location")]
        location: LocationFixture,
        #[serde(rename = "TransferDirection")]
        direction: &'static str,
    }

    #[derive(Serialize)]
    struct SpecificationFixture {
        #[serde(rename = "Identifier")]
        identifier: &'static str,
    }

    #[derive(Serialize)]
    struct LocationFixture {
        #[serde(rename = "Path")]
        path: &'static str,
        #[serde(rename = "FileExtension")]
        extension: &'static str,
    }

    struct RejectingWriter;

    impl Write for RejectingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("synthetic staging failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn manifest() -> Result<String, quick_xml::SeError> {
        quick_xml::se::to_string(&ManifestFixture {
            namespace: NAMESPACE,
            model: ModelFixture {
                version: 2244,
                description: "Synthetic Garmin",
            },
            id: 123_456,
            mass_storage: StorageFixture {
                data_type: DataTypeFixture {
                    name: "FIT_TYPE_4",
                    file: FileFixture {
                        specification: SpecificationFixture { identifier: "FIT" },
                        location: LocationFixture {
                            path: "GARMIN/ACTIVITY",
                            extension: "FIT",
                        },
                        direction: "OutputFromUnit",
                    },
                },
            },
        })
    }

    #[test]
    fn finds_manifest_below_a_storage_container() {
        assert_eq!(
            storage_prefix(Path::new("Internal Storage/GARMIN/GarminDevice.xml")).as_deref(),
            Some(Path::new("Internal Storage"))
        );
        assert_eq!(
            storage_prefix(Path::new("GARMIN/GarminDevice.xml")).as_deref(),
            Some(Path::new(""))
        );
        assert_eq!(storage_prefix(Path::new("GARMIN/Other.xml")), None);
    }

    #[test]
    fn scans_and_streams_local_gio_files() -> Result<(), Box<dyn StdError>> {
        let mount = tempdir()?;
        let storage = mount.path().join("Internal Storage");
        let garmin = storage.join("GARMIN");
        let activities = garmin.join("ACTIVITY");
        fs::create_dir_all(&activities)?;
        fs::write(garmin.join("GarminDevice.xml"), manifest()?)?;
        let source = activities.join("activity.fit");
        fs::write(&source, [1, 2, 3, 4])?;
        fs::write(activities.join("ignored.txt"), [5])?;

        let candidate = MountedMtpCandidate {
            mount_id: "test://synthetic".to_owned(),
            name: "Synthetic mount".to_owned(),
            root: File::for_path(mount.path()),
        };
        assert_eq!(candidate.attachment_key(), "test://synthetic");
        assert_eq!(candidate.display_name(), "Synthetic mount");
        assert!(format!("{candidate:?}").contains("Synthetic mount"));

        let catalogs = candidate.scan()?;
        let [catalog] = catalogs.as_slice() else {
            return Err("expected one manifest-bearing storage".into());
        };
        assert_eq!(catalog.manifest().model().description(), "Synthetic Garmin");
        let [file] = catalog.files() else {
            return Err("expected one readable file".into());
        };
        assert_eq!(file.data_type(), crate::capabilities::DataType::Activity);
        assert_eq!(
            file.direction(),
            crate::capabilities::TransferDirection::OutputFromUnit
        );
        assert_eq!(file.declared_size(), 4);
        assert!(format!("{file:?}").contains("declared_size"));

        let mut bytes = Vec::new();
        let outcome = file.copy_to(&mut bytes)?;
        assert_eq!(outcome.bytes(), 4);
        assert_eq!(bytes, [1, 2, 3, 4]);
        assert!(matches!(
            file.copy_to(&mut RejectingWriter),
            Err(Error::Target(_))
        ));

        fs::write(source, [1, 2])?;
        assert!(matches!(
            file.copy_to(&mut Vec::new()),
            Err(Error::SizeMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn rejects_missing_unsafe_and_oversized_mount_data() -> Result<(), Box<dyn StdError>> {
        let mount = tempdir()?;
        let candidate = MountedMtpCandidate {
            mount_id: "test://empty".to_owned(),
            name: "Empty mount".to_owned(),
            root: File::for_path(mount.path()),
        };
        assert!(matches!(candidate.scan(), Err(Error::ManifestMissing)));
        assert!(matches!(
            validate_component(Path::new("../unsafe")),
            Err(Error::UnsafeObjectName)
        ));

        let oversized = mount.path().join("oversized.xml");
        let file = fs::File::create(&oversized)?;
        file.set_len(super::MAX_MANIFEST_BYTES + 1)?;
        assert!(matches!(
            read_manifest(&File::for_path(oversized)),
            Err(Error::ManifestTooLarge)
        ));
        Ok(())
    }
}
