use garmin_model::map::{
    EmbeddedMapUnlock, InstallationState, MapAuthorization, MapCatalog, MapComponent, MapContent,
    MapDownload, MapFile, MapFileSegment, MapInstallIdentifier, MapInstallOption, MapUnlock,
    MapUnlockCode,
};

pub(crate) mod wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "PascalCase")]
    pub(crate) struct UpdateOptions {
        pub grouping_mode: &'static str,
        pub extraction_mode: &'static str,
    }

    impl Default for UpdateOptions {
        fn default() -> Self {
            Self {
                grouping_mode: "GroupByGeographicArea",
                extraction_mode: "ExtractIndependentSmallerRegions",
            }
        }
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "PascalCase")]
    pub(crate) struct GetUpdatesRequest<'a> {
        pub garmin_device_xml: &'a str,
        pub options: UpdateOptions,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct GetUpdatesResponse {
        pub maps: Vec<UniversalMap>,
        #[serde(default, deserialize_with = "deserialize_removed_files")]
        pub files_to_remove: Vec<FileToRemove>,
        #[serde(default, deserialize_with = "deserialize_directory_paths")]
        pub directories_to_remove: Vec<String>,
        #[serde(default)]
        pub bundled_maps: Vec<UniversalMap>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct UniversalMap {
        #[serde(default, deserialize_with = "deserialize_release")]
        pub release: Option<String>,
        pub display_name: String,
        #[serde(default)]
        pub map_type: Option<String>,
        #[serde(default, deserialize_with = "deserialize_removed_files")]
        pub files_to_remove: Vec<FileToRemove>,
        pub install_options: Vec<MapInstallOption>,
        #[serde(default)]
        pub is_reinstall: bool,
        #[serde(default)]
        pub can_uninstall: bool,
        #[serde(default)]
        pub installation_state: InstallationState,
        #[serde(default)]
        pub required_for_geographic_area_grouping: bool,
        #[serde(default, deserialize_with = "deserialize_download_hosts")]
        pub download_hosts: Vec<String>,
        #[serde(default, deserialize_with = "deserialize_geographic_region")]
        pub geographic_region: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FileToRemove {
        pub file_name: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pub part_number: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        pub size_in_bytes: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        pub is_shared: bool,
    }

    impl FileToRemove {
        #[must_use]
        pub fn path(path: impl Into<String>) -> Self {
            Self {
                file_name: path.into(),
                part_number: String::new(),
                size_in_bytes: 0,
                is_shared: false,
            }
        }
    }

    impl From<String> for FileToRemove {
        fn from(path: String) -> Self {
            Self::path(path)
        }
    }

    impl From<&str> for FileToRemove {
        fn from(path: &str) -> Self {
            Self::path(path)
        }
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde skip_serializing_if predicates receive field references"
    )]
    const fn is_zero(value: &u64) -> bool {
        *value == 0
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde skip_serializing_if predicates receive field references"
    )]
    const fn is_false(value: &bool) -> bool {
        !*value
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub enum InstallationState {
        #[default]
        NotInstalled,
        Installed,
        Partial,
        NewerInstalled,
    }

    impl InstallationState {
        #[must_use]
        pub const fn is_present(self) -> bool {
            !matches!(self, Self::NotInstalled)
        }

        const fn code(self) -> u8 {
            match self {
                Self::NotInstalled => 0,
                Self::Installed => 1,
                Self::Partial => 2,
                Self::NewerInstalled => 3,
            }
        }
    }

    impl Serialize for InstallationState {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_u8(self.code())
        }
    }

    impl<'de> Deserialize<'de> for InstallationState {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[derive(Deserialize)]
            #[serde(untagged)]
            enum Wire {
                Code(u8),
                Name(String),
            }

            match Wire::deserialize(deserializer)? {
                Wire::Code(0) => Ok(Self::NotInstalled),
                Wire::Code(1) => Ok(Self::Installed),
                Wire::Code(2) => Ok(Self::Partial),
                Wire::Code(3) => Ok(Self::NewerInstalled),
                Wire::Code(code) => Err(serde::de::Error::custom(format!(
                    "unknown Garmin map installation state {code}"
                ))),
                Wire::Name(name) => match name.to_ascii_lowercase().as_str() {
                    "notinstalled" | "not_installed" => Ok(Self::NotInstalled),
                    "installed" => Ok(Self::Installed),
                    "partial" => Ok(Self::Partial),
                    "newerinstalled" | "newer_installed" => Ok(Self::NewerInstalled),
                    _ => Err(serde::de::Error::custom(format!(
                        "unknown Garmin map installation state {name:?}"
                    ))),
                },
            }
        }
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct MapInstallOption {
        pub identifier: MapInstallIdentifier,
        #[serde(default)]
        pub display_name: String,
        #[serde(default)]
        pub preview_url: Option<String>,
        #[serde(default)]
        pub is_preferred: bool,
        pub files: Vec<Content>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct MapInstallIdentifier {
        #[serde(default)]
        pub region_part_number: Option<String>,
        #[serde(default)]
        pub map_image_part_number: Option<String>,
        #[serde(default)]
        pub activation_request_code: Option<String>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct Content {
        #[serde(default)]
        pub part_number: Option<String>,
        pub file_name: String,
        #[serde(default)]
        pub data_type: Option<String>,
        pub deliverable_options: Vec<DeliverableOption>,
        #[serde(default)]
        pub locale: Option<String>,
        #[serde(default)]
        pub external_file_name: Option<String>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct DeliverableOption {
        #[serde(alias = "MD5", alias = "Md5")]
        pub md5: String,
        pub size_in_bytes: u64,
        #[serde(
            default,
            rename(serialize = "Type", deserialize = "type"),
            deserialize_with = "deserialize_delivery_type"
        )]
        pub delivery_type: Option<String>,
        #[serde(alias = "URL", alias = "Url")]
        pub url: String,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ActivateUpdatesRequest<'a> {
        pub garmin_device_xml: &'a str,
        pub installed_maps: Vec<MapInstallIdentifierRequest<'a>>,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "PascalCase")]
    pub struct MapInstallIdentifierRequest<'a> {
        pub region_part_number: Option<&'a str>,
        pub map_image_part_number: Option<&'a str>,
        pub activation_request_code: Option<&'a str>,
    }

    impl<'a> From<&'a super::MapInstallIdentifier> for MapInstallIdentifierRequest<'a> {
        fn from(identifier: &'a super::MapInstallIdentifier) -> Self {
            Self {
                region_part_number: identifier.region_part_number.as_deref(),
                map_image_part_number: identifier.map_image_part_number.as_deref(),
                activation_request_code: identifier.activation_request_code.as_deref(),
            }
        }
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct ActivateUpdatesResponse {
        #[serde(default)]
        pub signed_sd_card_bytes: Option<String>,
        pub unlocks: Vec<ContentsUnlock>,
        #[serde(default)]
        pub embedded_unlocks: Vec<EmbeddedUnlock>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct ContentsUnlock {
        pub file_name: String,
        pub gma: String,
        #[serde(default)]
        pub codes: Vec<ContentsUnlockCode>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct ContentsUnlockCode {
        #[serde(default)]
        pub code: Option<String>,
        #[serde(default)]
        pub part_numbers: Option<Vec<String>>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct EmbeddedUnlock {
        pub part_number: String,
        #[serde(default)]
        pub segments: Vec<FileSegment>,
    }

    #[derive(Debug, Clone, Default, Deserialize, Serialize)]
    #[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
    pub struct FileSegment {
        pub offset: i64,
        pub data: String,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ReleaseWire {
        Text(String),
        Details(ReleaseDetailsWire),
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ReleaseDetailsWire {
        major_version: Option<u64>,
        minor_version: Option<u64>,
    }

    fn deserialize_release<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<ReleaseWire>::deserialize(deserializer)? {
            Some(ReleaseWire::Text(value)) => Some(value),
            Some(ReleaseWire::Details(details)) => {
                match (details.major_version, details.minor_version) {
                    (Some(major), Some(minor)) => Some(format!("{major}.{minor:02}")),
                    (Some(major), None) => Some(major.to_string()),
                    _ => None,
                }
            }
            None => None,
        })
    }

    impl<'de> Deserialize<'de> for FileToRemove {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Details {
                file_name: String,
                #[serde(default)]
                part_number: String,
                #[serde(default)]
                size_in_bytes: u64,
                #[serde(default)]
                is_shared: bool,
            }

            #[derive(Deserialize)]
            #[serde(untagged)]
            enum Wire {
                Text(String),
                Details(Details),
            }

            Ok(match Wire::deserialize(deserializer)? {
                Wire::Text(path) => Self::path(path),
                Wire::Details(details) => Self {
                    file_name: details.file_name,
                    part_number: details.part_number,
                    size_in_bytes: details.size_in_bytes,
                    is_shared: details.is_shared,
                },
            })
        }
    }

    fn deserialize_removed_files<'de, D>(deserializer: D) -> Result<Vec<FileToRemove>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Vec::<FileToRemove>::deserialize(deserializer)?
            .into_iter()
            .filter(|file| !file.file_name.is_empty())
            .collect())
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DirectoryPathWire {
        Text(String),
        Details(DirectoryPathDetailsWire),
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DirectoryPathDetailsWire {
        directory_name: String,
    }

    fn deserialize_directory_paths<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Vec::<DirectoryPathWire>::deserialize(deserializer)?
            .into_iter()
            .map(|path| match path {
                DirectoryPathWire::Text(path) => path,
                DirectoryPathWire::Details(path) => path.directory_name,
            })
            .filter(|path| !path.is_empty())
            .collect())
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DownloadHostsWire {
        List(Vec<String>),
        Details(DownloadHostDetailsWire),
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DownloadHostDetailsWire {
        foreground_primary_host: Option<String>,
        background_primary_host: Option<String>,
        #[serde(default)]
        failover_hosts: Vec<String>,
    }

    fn deserialize_download_hosts<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(hosts) = Option::<DownloadHostsWire>::deserialize(deserializer)? else {
            return Ok(Vec::new());
        };
        let hosts = match hosts {
            DownloadHostsWire::List(hosts) => hosts,
            DownloadHostsWire::Details(details) => details
                .foreground_primary_host
                .into_iter()
                .chain(details.background_primary_host)
                .chain(details.failover_hosts)
                .collect(),
        };
        let mut unique = Vec::with_capacity(hosts.len());
        for host in hosts {
            if !host.is_empty() && !unique.contains(&host) {
                unique.push(host);
            }
        }
        Ok(unique)
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum GeographicRegionWire {
        Text(String),
        Details(GeographicRegionDetailsWire),
    }

    #[derive(Deserialize)]
    struct GeographicRegionDetailsWire {
        title: String,
    }

    fn deserialize_geographic_region<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            match Option::<GeographicRegionWire>::deserialize(deserializer)? {
                Some(GeographicRegionWire::Text(region)) => Some(region),
                Some(GeographicRegionWire::Details(region)) => Some(region.title),
                None => None,
            }
            .filter(|region| !region.is_empty()),
        )
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DeliveryTypeWire {
        Text(String),
        Code(u64),
    }

    fn deserialize_delivery_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            match Option::<DeliveryTypeWire>::deserialize(deserializer)? {
                Some(DeliveryTypeWire::Text(value)) => Some(value),
                Some(DeliveryTypeWire::Code(1)) => Some("Full".to_owned()),
                Some(DeliveryTypeWire::Code(code)) => Some(code.to_string()),
                None => None,
            },
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use garmin_model::map::{InstallationState, MapCatalog, MapFile};

        #[derive(Serialize)]
        #[serde(rename = "Device")]
        struct EmptyDevice;

        #[test]
        fn serializes_exact_update_contract() {
            let device_xml = quick_xml::se::to_string(&EmptyDevice).unwrap();
            let body = serde_json::to_value(GetUpdatesRequest {
                garmin_device_xml: &device_xml,
                options: UpdateOptions::default(),
            })
            .unwrap();
            assert_eq!(body["GarminDeviceXml"], device_xml);
            assert_eq!(body["Options"]["GroupingMode"], "GroupByGeographicArea");
        }

        #[test]
        fn parses_update_response_shape() {
            let value = serde_json::json!({
                "maps": [{
                    "release": {
                        "majorVersion": 2026,
                        "minorVersion": 11
                    },
                    "displayName": "Example Regional Map",
                    "installationState": 1,
                    "requiredForGeographicAreaGrouping": true,
                    "filesToRemove": [{
                        "fileName": "Garmin/Maps/old.img",
                        "sizeInBytes": 4
                    }],
                    "downloadHosts": {
                        "foregroundPrimaryHost": "https://omtmapupdate.garmin.com/",
                        "backgroundPrimaryHost": "https://omtmapupdate.garmin.com/",
                        "failoverHosts": [
                            "http://downloadg.garmin.com/",
                            "https://omtmapupdate.garmin.com/"
                        ]
                    },
                    "geographicRegion": {
                        "regionCode": "EU",
                        "title": "Europe"
                    },
                    "installOptions": [{
                        "identifier": {
                            "regionPartNumber": "MOCK-REGION-01",
                            "mapImagePartNumber": "MOCK-MAP-01",
                            "activationRequestCode": "request"
                        },
                        "isPreferred": true,
                        "files": [{
                            "fileName": "Garmin/Maps/map.img",
                            "externalFileName": "",
                            "deliverableOptions": [{
                                "md5": "1ycfcokOgTWhTaQIoLP2bQ==",
                                "sizeInBytes": 4,
                                "type": 1,
                                "url": "rmu/maps/map.img"
                            }]
                        }]
                    }]
                }],
                "filesToRemove": [],
                "directoriesToRemove": [],
                "bundledMaps": []
            });
            let parsed: MapCatalog = serde_json::from_value::<GetUpdatesResponse>(value)
                .unwrap()
                .into();
            assert_eq!(parsed.maps.len(), 1);
            assert_eq!(parsed.maps[0].release.as_deref(), Some("2026.11"));
            assert_eq!(
                parsed.maps[0].installation_state,
                InstallationState::Installed
            );
            assert!(parsed.maps[0].required_for_geographic_area_grouping);
            assert_eq!(
                parsed.maps[0].files_to_remove,
                [MapFile {
                    file_name: "Garmin/Maps/old.img".to_owned(),
                    part_number: String::new(),
                    size_in_bytes: 4,
                    is_shared: false,
                }]
            );
            assert_eq!(parsed.maps[0].download_hosts.len(), 2);
            assert_eq!(parsed.maps[0].geographic_region.as_deref(), Some("Europe"));
            assert_eq!(
                parsed.maps[0].install_options[0]
                    .identifier
                    .region_part_number
                    .as_deref(),
                Some("MOCK-REGION-01")
            );
            let delivery = &parsed.maps[0].install_options[0].files[0].downloads[0];
            assert_eq!(delivery.delivery_type.as_deref(), Some("Full"));
            assert_eq!(delivery.url, "rmu/maps/map.img");
        }

        #[test]
        fn rejects_an_unrecognized_response_instead_of_reporting_no_maps() {
            let error = serde_json::from_value::<GetUpdatesResponse>(serde_json::json!({
                "Maps": []
            }))
            .unwrap_err();

            assert!(error.to_string().contains("missing field `maps`"));
        }
    }
}

impl From<wire::GetUpdatesResponse> for MapCatalog {
    fn from(response: wire::GetUpdatesResponse) -> Self {
        Self {
            maps: response.maps.into_iter().map(Into::into).collect(),
            files_to_remove: response
                .files_to_remove
                .into_iter()
                .map(Into::into)
                .collect(),
            directories_to_remove: response.directories_to_remove,
            bundled_maps: response.bundled_maps.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wire::UniversalMap> for MapComponent {
    fn from(map: wire::UniversalMap) -> Self {
        Self {
            release: map.release,
            installed_version: None,
            display_name: map.display_name,
            map_type: map.map_type,
            files_to_remove: map.files_to_remove.into_iter().map(Into::into).collect(),
            install_options: map.install_options.into_iter().map(Into::into).collect(),
            is_reinstall: map.is_reinstall,
            can_uninstall: map.can_uninstall,
            installation_state: map.installation_state.into(),
            required_for_geographic_area_grouping: map.required_for_geographic_area_grouping,
            download_hosts: map.download_hosts,
            geographic_region: map.geographic_region,
        }
    }
}

impl From<wire::FileToRemove> for MapFile {
    fn from(file: wire::FileToRemove) -> Self {
        Self {
            file_name: file.file_name,
            part_number: file.part_number,
            size_in_bytes: file.size_in_bytes,
            is_shared: file.is_shared,
        }
    }
}

impl From<wire::InstallationState> for InstallationState {
    fn from(state: wire::InstallationState) -> Self {
        match state {
            wire::InstallationState::NotInstalled => Self::NotInstalled,
            wire::InstallationState::Installed => Self::Installed,
            wire::InstallationState::Partial => Self::Partial,
            wire::InstallationState::NewerInstalled => Self::NewerInstalled,
        }
    }
}

impl From<wire::MapInstallOption> for MapInstallOption {
    fn from(option: wire::MapInstallOption) -> Self {
        Self {
            identifier: option.identifier.into(),
            display_name: option.display_name,
            preview_url: option.preview_url,
            is_preferred: option.is_preferred,
            files: option.files.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wire::MapInstallIdentifier> for MapInstallIdentifier {
    fn from(identifier: wire::MapInstallIdentifier) -> Self {
        Self {
            region_part_number: identifier.region_part_number,
            map_image_part_number: identifier.map_image_part_number,
            activation_request_code: identifier.activation_request_code,
        }
    }
}

impl From<wire::Content> for MapContent {
    fn from(content: wire::Content) -> Self {
        Self {
            part_number: content.part_number,
            file_name: content.file_name,
            data_type: content.data_type,
            downloads: content
                .deliverable_options
                .into_iter()
                .map(Into::into)
                .collect(),
            locale: content.locale,
            external_file_name: content.external_file_name,
        }
    }
}

impl From<wire::DeliverableOption> for MapDownload {
    fn from(option: wire::DeliverableOption) -> Self {
        Self {
            md5: option.md5,
            size_in_bytes: option.size_in_bytes,
            delivery_type: option.delivery_type,
            url: option.url,
        }
    }
}

impl From<wire::ActivateUpdatesResponse> for MapAuthorization {
    fn from(response: wire::ActivateUpdatesResponse) -> Self {
        Self {
            signed_sd_card_bytes: response.signed_sd_card_bytes,
            unlocks: response.unlocks.into_iter().map(Into::into).collect(),
            embedded_unlocks: response
                .embedded_unlocks
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<wire::ContentsUnlock> for MapUnlock {
    fn from(unlock: wire::ContentsUnlock) -> Self {
        Self {
            file_name: unlock.file_name,
            gma: unlock.gma,
            codes: unlock.codes.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wire::ContentsUnlockCode> for MapUnlockCode {
    fn from(code: wire::ContentsUnlockCode) -> Self {
        Self {
            code: code.code,
            part_numbers: code.part_numbers,
        }
    }
}

impl From<wire::EmbeddedUnlock> for EmbeddedMapUnlock {
    fn from(unlock: wire::EmbeddedUnlock) -> Self {
        Self {
            part_number: unlock.part_number,
            segments: unlock.segments.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wire::FileSegment> for MapFileSegment {
    fn from(segment: wire::FileSegment) -> Self {
        Self {
            offset: segment.offset,
            data: segment.data,
        }
    }
}
