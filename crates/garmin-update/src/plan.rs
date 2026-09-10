use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use garmin_device::{PathSafetyError, SafeRelativePath};
use garmin_model::map::{MapCatalog, MapComponent, MapDownload, MapInstallIdentifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;
use url::Url;

pub const UPDATE_PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupPolicy {
    #[default]
    Verified,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadSpec {
    pub map_name: String,
    pub source: Url,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternate_sources: Vec<Url>,
    #[serde(default)]
    pub requires_garmin_token: bool,
    pub destination: SafeRelativePath,
    pub cache_name: String,
    pub size: u64,
    pub md5: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePlan {
    pub schema_version: u32,
    pub device_digest: String,
    pub downloads: Vec<DownloadSpec>,
    pub files_to_remove: Vec<SafeRelativePath>,
    pub identifiers: Vec<MapInstallIdentifier>,
    pub total_bytes: u64,
    pub backup_policy: BackupPolicy,
    pub digest: String,
}

#[derive(Serialize)]
struct PlanDigest<'a> {
    device_digest: &'a str,
    downloads: Vec<DownloadDigest<'a>>,
    files_to_remove: &'a [SafeRelativePath],
    identifiers: &'a [MapInstallIdentifier],
    total_bytes: u64,
    backup_policy: BackupPolicy,
}

#[derive(Serialize)]
struct DownloadDigest<'a> {
    map_name: &'a str,
    source: String,
    alternate_sources: Vec<String>,
    requires_garmin_token: bool,
    destination: &'a SafeRelativePath,
    size: u64,
    md5: &'a str,
}

impl UpdatePlan {
    /// Plan every map using its preferred install option.
    /// # Errors
    /// [`UpdatePlanError`] for incomplete content or unsafe paths.
    /// It also rejects insecure URLs and serialization failures.
    pub fn from_response(
        response: &MapCatalog,
        device_digest: String,
    ) -> Result<Self, UpdatePlanError> {
        let map_count = response.maps.len() + response.bundled_maps.len();
        Self::build(response, device_digest, 0..map_count, None)
    }

    /// Plan only the selected map components.
    ///
    /// Indexes follow `Maps`, then `BundledMaps`. Response-wide removals require
    /// every offered component because the service omits their owner.
    /// # Errors
    /// Invalid selection or content, unsafe path, insecure URL, or serialization failure.
    pub fn from_response_selection(
        response: &MapCatalog,
        device_digest: String,
        selected_maps: impl IntoIterator<Item = usize>,
    ) -> Result<Self, UpdatePlanError> {
        Self::build(response, device_digest, selected_maps, None)
    }

    /// Plan selected maps from one literal loopback origin.
    /// # Errors
    /// [`UpdatePlanError`] for a non-loopback origin, a cross-origin
    /// deliverable, or any normal plan-validation error.
    pub fn from_mock_response_selection(
        response: &MapCatalog,
        device_digest: String,
        selected_maps: impl IntoIterator<Item = usize>,
        mock_base: &Url,
    ) -> Result<Self, UpdatePlanError> {
        if !is_http_loopback_url(mock_base) {
            return Err(UpdatePlanError::NonLoopbackMock(mock_base.clone()));
        }
        Self::build(response, device_digest, selected_maps, Some(mock_base))
    }

    fn build(
        response: &MapCatalog,
        device_digest: String,
        selected_maps: impl IntoIterator<Item = usize>,
        mock_base: Option<&Url>,
    ) -> Result<Self, UpdatePlanError> {
        let mut downloads = Vec::new();
        let mut identifiers = Vec::new();
        let mut files_to_remove = Vec::new();
        let selected = selected_maps.into_iter().collect::<BTreeSet<_>>();
        let maps = response
            .maps
            .iter()
            .chain(&response.bundled_maps)
            .collect::<Vec<_>>();

        if let Some(index) = selected.iter().find(|index| **index >= maps.len()) {
            return Err(UpdatePlanError::UnknownMapIndex(*index));
        }

        if !maps.is_empty() && selected.len() == maps.len() {
            if !response.directories_to_remove.is_empty() {
                return Err(UpdatePlanError::DirectoryRemovalUnsupported(
                    response.directories_to_remove.len(),
                ));
            }
            for file in &response.files_to_remove {
                files_to_remove.push(SafeRelativePath::parse(&file.file_name)?);
            }
        }

        for (index, map) in maps.into_iter().enumerate() {
            if !selected.contains(&index) {
                continue;
            }
            append_map(
                map,
                mock_base,
                &mut downloads,
                &mut identifiers,
                &mut files_to_remove,
            )?;
        }

        files_to_remove.sort_by(|left, right| left.as_path().cmp(right.as_path()));
        files_to_remove.dedup();
        let total_bytes = downloads.iter().try_fold(0_u64, |total, item| {
            total
                .checked_add(item.size)
                .ok_or(UpdatePlanError::TotalSizeOverflow)
        })?;
        let mut plan = Self {
            schema_version: UPDATE_PLAN_SCHEMA_VERSION,
            device_digest,
            downloads,
            files_to_remove,
            identifiers,
            total_bytes,
            backup_policy: BackupPolicy::default(),
            digest: String::new(),
        };
        plan.digest = plan.calculated_digest()?;
        Ok(plan)
    }

    /// Validate a plan loaded from a capture.
    /// # Errors
    /// Unsupported schemas, inconsistent totals, or a changed digest.
    pub fn validate(&self) -> Result<(), UpdatePlanError> {
        if self.schema_version != UPDATE_PLAN_SCHEMA_VERSION {
            return Err(UpdatePlanError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        for download in &self.downloads {
            if normalized_md5(&download.md5).as_deref() != Some(download.cache_name.as_str()) {
                return Err(UpdatePlanError::InvalidCacheAddress(
                    download.cache_name.clone(),
                ));
            }
        }
        let total_bytes = self.downloads.iter().try_fold(0_u64, |total, item| {
            total
                .checked_add(item.size)
                .ok_or(UpdatePlanError::TotalSizeOverflow)
        })?;
        if total_bytes != self.total_bytes {
            return Err(UpdatePlanError::TotalSizeMismatch {
                declared: self.total_bytes,
                calculated: total_bytes,
            });
        }
        let calculated = self.calculated_digest()?;
        if calculated != self.digest {
            return Err(UpdatePlanError::DigestMismatch {
                declared: self.digest.clone(),
                calculated,
            });
        }
        Ok(())
    }

    /// Return this plan with the requested recovery-backup policy fingerprinted
    /// into its identity.
    /// # Errors
    /// Serialization of the canonical plan representation failed.
    pub fn with_backup_policy(mut self, policy: BackupPolicy) -> Result<Self, UpdatePlanError> {
        self.schema_version = UPDATE_PLAN_SCHEMA_VERSION;
        self.backup_policy = policy;
        self.digest = self.calculated_digest()?;
        Ok(self)
    }

    fn download_digests(&self) -> Vec<DownloadDigest<'_>> {
        self.downloads
            .iter()
            .map(|download| DownloadDigest {
                map_name: &download.map_name,
                source: stable_source(&download.source),
                alternate_sources: download
                    .alternate_sources
                    .iter()
                    .map(stable_source)
                    .collect(),
                requires_garmin_token: download.requires_garmin_token,
                destination: &download.destination,
                size: download.size,
                md5: &download.md5,
            })
            .collect()
    }

    fn calculated_digest(&self) -> Result<String, UpdatePlanError> {
        let canonical = serde_json::to_vec(&PlanDigest {
            device_digest: &self.device_digest,
            downloads: self.download_digests(),
            files_to_remove: &self.files_to_remove,
            identifiers: &self.identifiers,
            total_bytes: self.total_bytes,
            backup_policy: self.backup_policy,
        })?;
        Ok(sha256_hex(&canonical))
    }
}

fn append_map(
    map: &MapComponent,
    mock_base: Option<&Url>,
    downloads: &mut Vec<DownloadSpec>,
    identifiers: &mut Vec<MapInstallIdentifier>,
    files_to_remove: &mut Vec<SafeRelativePath>,
) -> Result<(), UpdatePlanError> {
    for file in &map.files_to_remove {
        files_to_remove.push(SafeRelativePath::parse(&file.file_name)?);
    }
    let option = map
        .install_options
        .iter()
        .find(|option| option.is_preferred)
        .or_else(|| map.install_options.first())
        .ok_or_else(|| UpdatePlanError::NoInstallOption(map.display_name.clone()))?;
    identifiers.push(option.identifier.clone());

    for content in &option.files {
        let delivery = preferred_delivery(&content.downloads).ok_or_else(|| {
            UpdatePlanError::NoDeliverable {
                map: map.display_name.clone(),
                file: content.file_name.clone(),
            }
        })?;
        if delivery.size_in_bytes == 0 {
            return Err(UpdatePlanError::EmptyDeliverable {
                map: map.display_name.clone(),
                file: content.file_name.clone(),
            });
        }
        let Some(md5) = normalized_md5(&delivery.md5) else {
            return Err(UpdatePlanError::InvalidChecksum {
                map: map.display_name.clone(),
                file: content.file_name.clone(),
            });
        };
        let sources = validated_sources(&delivery.url, &map.download_hosts, mock_base)?;
        let mut sources = sources.into_iter();
        let source = sources
            .next()
            .expect("validated sources always contain a primary URL");
        let destination_name = content
            .external_file_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&content.file_name);
        let destination = SafeRelativePath::parse(destination_name)?;
        let cache_name = md5.clone();
        let candidate = DownloadSpec {
            map_name: map.display_name.clone(),
            source,
            alternate_sources: sources.collect(),
            requires_garmin_token: Url::parse(&delivery.url).is_err(),
            destination,
            cache_name,
            size: delivery.size_in_bytes,
            md5,
        };
        if let Some(existing) = downloads
            .iter()
            .find(|existing| existing.destination == candidate.destination)
        {
            if same_artifact(existing, &candidate) {
                continue;
            }
            return Err(UpdatePlanError::DuplicateDestination(candidate.destination));
        }
        downloads.push(candidate);
        if downloads.len() > 512 {
            return Err(UpdatePlanError::TooManyDownloads);
        }
    }
    Ok(())
}

fn same_artifact(left: &DownloadSpec, right: &DownloadSpec) -> bool {
    left.size == right.size && left.md5 == right.md5
}

fn validated_sources(
    value: &str,
    download_hosts: &[String],
    mock_base: Option<&Url>,
) -> Result<Vec<Url>, UpdatePlanError> {
    let sources = match Url::parse(value) {
        Ok(source) => vec![source],
        Err(url::ParseError::RelativeUrlWithoutBase)
            if mock_base.is_none() && !value.starts_with("//") =>
        {
            let mut sources = Vec::new();
            for base in download_hosts
                .iter()
                .filter_map(|host| Url::parse(host).ok())
                .filter(|host| host.scheme() == "https")
            {
                let source = base.join(value)?;
                if !sources.contains(&source) {
                    sources.push(source);
                }
            }
            if sources.is_empty() {
                return Err(UpdatePlanError::MissingDownloadHost(value.to_owned()));
            }
            sources
        }
        Err(error) => return Err(error.into()),
    };
    if let Some(mock_base) = mock_base {
        if let Some(source) = sources
            .iter()
            .find(|source| source.origin() != mock_base.origin())
        {
            return Err(UpdatePlanError::MockOriginEscape(source.clone()));
        }
    } else if let Some(source) = sources.iter().find(|source| source.scheme() != "https") {
        return Err(UpdatePlanError::InsecureUrl(source.clone()));
    }
    Ok(sources)
}

fn normalized_md5(value: &str) -> Option<String> {
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(value.to_ascii_lowercase());
    }
    let digest = BASE64.decode(value).ok()?;
    (digest.len() == 16).then(|| hex::encode(digest))
}

fn stable_source(source: &Url) -> String {
    let mut source = source.clone();
    source.set_query(None);
    source.set_fragment(None);
    source.into()
}

fn is_http_loopback_url(url: &Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    }
}

fn preferred_delivery(options: &[MapDownload]) -> Option<&MapDownload> {
    options
        .iter()
        .find(|option| option.delivery_type.as_deref() == Some("Full"))
        .or_else(|| options.first())
}

fn sha256_hex(input: &[u8]) -> String {
    hex::encode(Sha256::digest(input))
}

#[derive(Debug, Error)]
pub enum UpdatePlanError {
    #[error("update plan schema {0} is not supported")]
    UnsupportedSchemaVersion(u32),
    #[error("update plan declares {declared} bytes but contains {calculated} bytes")]
    TotalSizeMismatch { declared: u64, calculated: u64 },
    #[error("update plan digest {declared} does not match {calculated}")]
    DigestMismatch {
        declared: String,
        calculated: String,
    },
    #[error("map selection index {0} is outside the service response")]
    UnknownMapIndex(usize),
    #[error("mock service URL must use HTTP on loopback: {0}")]
    NonLoopbackMock(Url),
    #[error("mock deliverable escaped the configured service origin: {0}")]
    MockOriginEscape(Url),
    #[error(transparent)]
    UnsafePath(#[from] PathSafetyError),
    #[error("map {0} has no install option")]
    NoInstallOption(String),
    #[error("map {map} file {file} has no deliverable")]
    NoDeliverable { map: String, file: String },
    #[error("map {map} file {file} has an empty deliverable")]
    EmptyDeliverable { map: String, file: String },
    #[error("map {map} file {file} has no valid MD5 checksum")]
    InvalidChecksum { map: String, file: String },
    #[error("download cache address is not its normalized MD5: {0}")]
    InvalidCacheAddress(String),
    #[error("multiple downloads target {0}")]
    DuplicateDestination(SafeRelativePath),
    #[error("invalid content URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("relative content URL has no HTTPS Garmin download host: {0}")]
    MissingDownloadHost(String),
    #[error("content URL is not HTTPS: {0}")]
    InsecureUrl(Url),
    #[error("unable to serialize canonical update plan: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("total update size exceeds the supported range")]
    TotalSizeOverflow,
    #[error("update plan contains more than 512 downloads")]
    TooManyDownloads,
    #[error("update requires removing {0} directories, which is not yet supported")]
    DirectoryRemovalUnsupported(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_model::map::{MapComponent, MapContent, MapInstallOption};

    fn plan_fixture() -> UpdatePlan {
        UpdatePlan::from_response(
            &MapCatalog {
                maps: vec![MapComponent {
                    display_name: "Fixture map".to_owned(),
                    install_options: vec![MapInstallOption {
                        is_preferred: true,
                        files: vec![MapContent {
                            file_name: "Garmin/map.img".to_owned(),
                            downloads: vec![MapDownload {
                                md5: "00".repeat(16),
                                size_in_bytes: 10,
                                delivery_type: Some("Full".to_owned()),
                                url: "https://download.garmin.com/map.img".to_owned(),
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
            "device".to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn backup_policy_changes_the_current_plan_identity() {
        let plan = plan_fixture();
        let verified = plan
            .clone()
            .with_backup_policy(BackupPolicy::Verified)
            .unwrap();
        let skipped = plan.with_backup_policy(BackupPolicy::Skip).unwrap();

        assert_eq!(verified.schema_version, UPDATE_PLAN_SCHEMA_VERSION);
        assert_eq!(skipped.schema_version, UPDATE_PLAN_SCHEMA_VERSION);
        assert_ne!(verified.digest, skipped.digest);
        verified.validate().unwrap();
        skipped.validate().unwrap();
    }

    #[test]
    fn rejects_changed_or_unknown_persisted_plans() {
        let mut plan = plan_fixture();
        plan.total_bytes += 1;
        assert!(matches!(
            plan.validate(),
            Err(UpdatePlanError::TotalSizeMismatch { .. })
        ));

        plan.total_bytes -= 1;
        plan.schema_version = UPDATE_PLAN_SCHEMA_VERSION + 1;
        assert!(matches!(
            plan.validate(),
            Err(UpdatePlanError::UnsupportedSchemaVersion(_))
        ));
    }

    #[test]
    fn rejects_cache_names_that_are_not_content_addresses() {
        let mut plan = plan_fixture();
        plan.downloads[0].cache_name = "friendly-map-name".to_owned();

        assert!(matches!(
            plan.validate(),
            Err(UpdatePlanError::InvalidCacheAddress(_))
        ));
    }

    #[test]
    fn rejects_service_path_traversal() {
        let response = MapCatalog {
            maps: vec![MapComponent {
                display_name: "Example Map".to_owned(),
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "../escape.img".to_owned(),
                        downloads: vec![MapDownload {
                            md5: "00".repeat(16),
                            size_in_bytes: 10,
                            url: "https://download.garmin.com/map.img".to_owned(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(UpdatePlan::from_response(&response, "device".to_owned()).is_err());
    }

    #[test]
    fn resolves_relative_urls_and_base64_md5_values() {
        let response = MapCatalog {
            maps: vec![MapComponent {
                display_name: "Example Map".to_owned(),
                download_hosts: vec![
                    "https://omtmpaupdate.garmin.cn/".to_owned(),
                    "https://omtmapupdate.garmin.com/".to_owned(),
                ],
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "Garmin/Maps/map.img".to_owned(),
                        external_file_name: Some(String::new()),
                        downloads: vec![MapDownload {
                            md5: "AAAAAAAAAAAAAAAAAAAAAA==".to_owned(),
                            size_in_bytes: 10,
                            delivery_type: Some("Full".to_owned()),
                            url: "rmu/maps/map.img".to_owned(),
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let plan = UpdatePlan::from_response(&response, "device".to_owned()).unwrap();

        assert_eq!(
            plan.downloads[0].source.as_str(),
            "https://omtmpaupdate.garmin.cn/rmu/maps/map.img"
        );
        assert_eq!(
            plan.downloads[0].alternate_sources[0].as_str(),
            "https://omtmapupdate.garmin.com/rmu/maps/map.img"
        );
        assert!(plan.downloads[0].requires_garmin_token);
        assert_eq!(plan.downloads[0].md5, "00000000000000000000000000000000");
        assert_eq!(
            plan.downloads[0].destination.as_path(),
            std::path::Path::new("Garmin/Maps/map.img")
        );
    }

    #[test]
    fn plans_only_selected_map_components() {
        let map = |name: &str, file: &str, remove: &str| MapComponent {
            display_name: name.to_owned(),
            files_to_remove: vec![remove.into()],
            install_options: vec![MapInstallOption {
                is_preferred: true,
                files: vec![MapContent {
                    file_name: file.to_owned(),
                    downloads: vec![MapDownload {
                        md5: "00".repeat(16),
                        size_in_bytes: 10,
                        url: format!("https://download.garmin.com/{file}"),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let response = MapCatalog {
            maps: vec![
                map("Installed map", "installed.img", "old-installed.img"),
                map("Optional map", "optional.img", "old-optional.img"),
            ],
            files_to_remove: vec!["global-old.img".into()],
            ..Default::default()
        };

        let plan =
            UpdatePlan::from_response_selection(&response, "device".to_owned(), [1]).unwrap();
        assert_eq!(plan.downloads.len(), 1);
        assert_eq!(plan.downloads[0].map_name, "Optional map");
        assert_eq!(plan.files_to_remove.len(), 1);
        assert_eq!(
            plan.files_to_remove[0].as_path(),
            std::path::Path::new("old-optional.img")
        );
        assert_eq!(plan.identifiers.len(), 1);
    }

    #[test]
    fn full_selection_includes_response_wide_removals() {
        let response = MapCatalog {
            maps: vec![MapComponent {
                display_name: "Map".to_owned(),
                files_to_remove: vec!["map-old.img".into()],
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "map.img".to_owned(),
                        downloads: vec![MapDownload {
                            md5: "00".repeat(16),
                            size_in_bytes: 10,
                            url: "https://download.garmin.com/map.img".to_owned(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            files_to_remove: vec!["global-old.img".into()],
            ..Default::default()
        };

        let plan = UpdatePlan::from_response(&response, "device".to_owned()).unwrap();
        assert_eq!(plan.files_to_remove.len(), 2);
    }

    #[test]
    fn mock_plans_cannot_escape_the_loopback_origin() {
        let response_for = |url: &str| MapCatalog {
            maps: vec![MapComponent {
                display_name: "Mock map".to_owned(),
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "mock.img".to_owned(),
                        downloads: vec![MapDownload {
                            md5: "00".repeat(16),
                            size_in_bytes: 10,
                            url: url.to_owned(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let base = Url::parse("http://127.0.0.1:39765/").unwrap();
        assert!(
            UpdatePlan::from_mock_response_selection(
                &response_for("http://127.0.0.1:39765/downloads/mock.img"),
                "device".to_owned(),
                [0],
                &base,
            )
            .is_ok()
        );
        assert!(
            UpdatePlan::from_mock_response_selection(
                &response_for("https://download.garmin.com/mock.img"),
                "device".to_owned(),
                [0],
                &base,
            )
            .is_err()
        );
    }

    #[test]
    fn collapses_shared_artifacts_and_rejects_conflicting_destinations() {
        let content = |file: &str, destination: &str, md5: &str| MapContent {
            file_name: file.to_owned(),
            external_file_name: Some(destination.to_owned()),
            downloads: vec![MapDownload {
                md5: md5.to_owned(),
                size_in_bytes: 10,
                url: format!("https://download.garmin.com/{file}"),
                ..Default::default()
            }],
            ..Default::default()
        };
        let response = |files| MapCatalog {
            maps: vec![MapComponent {
                display_name: "Map".to_owned(),
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert!(matches!(
            UpdatePlan::from_response(
                &response(vec![content("one.img", "one.img", "")]),
                "device".to_owned()
            ),
            Err(UpdatePlanError::InvalidChecksum { .. })
        ));
        let mut first = response(vec![content("one.img", "same.img", &"00".repeat(16))]);
        let second = response(vec![content(
            "one-mirror.img",
            "same.img",
            &"00".repeat(16),
        )]);
        first.maps.extend(second.maps);
        let plan =
            UpdatePlan::from_response_selection(&first, "device".to_owned(), [0, 1]).unwrap();
        assert_eq!(plan.downloads.len(), 1);
        assert_eq!(plan.total_bytes, 10);
        assert_eq!(plan.identifiers.len(), 2);
        assert!(matches!(
            UpdatePlan::from_response(
                &response(vec![
                    content("one.img", "same.img", &"00".repeat(16)),
                    content("two.img", "same.img", &"11".repeat(16)),
                ]),
                "device".to_owned()
            ),
            Err(UpdatePlanError::DuplicateDestination(_))
        ));
    }

    #[test]
    fn plan_digest_ignores_ephemeral_download_urls() {
        let response = |url: &str| MapCatalog {
            maps: vec![MapComponent {
                display_name: "Map".to_owned(),
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "map.img".to_owned(),
                        downloads: vec![MapDownload {
                            md5: "00".repeat(16),
                            size_in_bytes: 10,
                            url: url.to_owned(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let first = UpdatePlan::from_response(
            &response("https://download.garmin.com/map.img?token=one"),
            "device".to_owned(),
        )
        .unwrap();
        let second = UpdatePlan::from_response(
            &response("https://download.garmin.com/map.img?token=two"),
            "device".to_owned(),
        )
        .unwrap();

        assert_eq!(first.digest, second.digest);
    }

    #[test]
    fn plan_digest_binds_the_download_origin_and_path() {
        let response_for = |url: &str| MapCatalog {
            maps: vec![MapComponent {
                display_name: "Map".to_owned(),
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "map.img".to_owned(),
                        downloads: vec![MapDownload {
                            md5: "00".repeat(16),
                            size_in_bytes: 10,
                            url: url.to_owned(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let first = UpdatePlan::from_response(
            &response_for("https://download.garmin.com/maps/map.img?token=one"),
            "device".to_owned(),
        )
        .unwrap();
        let second = UpdatePlan::from_response(
            &response_for("https://cdn.garmin.com/maps/map.img?token=one"),
            "device".to_owned(),
        )
        .unwrap();
        let third = UpdatePlan::from_response(
            &response_for("https://download.garmin.com/maps/other.img?token=one"),
            "device".to_owned(),
        )
        .unwrap();

        assert_ne!(first.digest, second.digest);
        assert_ne!(first.digest, third.digest);
    }
}
