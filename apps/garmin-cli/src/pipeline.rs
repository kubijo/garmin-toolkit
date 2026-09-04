use crate::{MapService, TargetArgs, build_update_plan, cache_dir, load_target, omt_client};
use anyhow::{Context, Result, bail};
use garmin_capture::SessionCapture;
use garmin_device::{
    DeviceManifest, DeviceProbeReport, DeviceSummary, MountedMtpDevice, RawMtpLink, RawMtpSession,
    storage::{DeviceLink, DeviceProbeRequest, DirectoryDevice},
};
use garmin_map_service::GARMIN_EXPRESS_USER_AGENT;
use garmin_model::map::MapCatalog;
use garmin_progress::{OperationStage, ProgressReporter};
use garmin_services::maps::GarminDownloadAuthorizer;
use garmin_update::{DownloadSpec, Downloader, space::StorageSpaceRequirement};
use serde::Serialize;
use std::time::{Duration, Instant};
use std::{path::PathBuf, sync::Arc};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct PipelineReport {
    pub device: DeviceSummary,
    pub map_name: String,
    pub destination: PathBuf,
    pub bytes: u64,
    pub inspect_seconds: f64,
    pub query_seconds: f64,
    pub planning_seconds: f64,
    pub download_seconds: f64,
    pub download_bytes_per_second: f64,
    pub md5_verified: bool,
    pub device_probe: DeviceProbeReport,
    pub total_seconds: f64,
    pub host_staging_removed: bool,
}

pub async fn run(
    target: TargetArgs,
    map_filter: Option<String>,
    progress: ProgressReporter,
    mock_server: Option<Url>,
    capture: Option<SessionCapture>,
) -> Result<PipelineReport> {
    let total_started = Instant::now();
    let (manifest, inspect_elapsed) = inspect_device(&target, &progress, capture.as_ref()).await?;
    let device_identity = manifest.identity_digest();

    let (response, query_elapsed) =
        query_updates(&manifest, &progress, mock_server.as_ref(), capture.clone()).await?;

    progress.started(
        OperationStage::Plan,
        "Selecting one real update file",
        Some(1),
    );
    let planning_started = Instant::now();
    let (plan, selected_index) = select_probe_plan(
        &response,
        &device_identity,
        map_filter.as_deref(),
        MapService::from_mock_base(mock_server.as_ref()),
    )
    .inspect_err(|error| {
        progress.failed(
            OperationStage::Plan,
            format!("Update planning failed: {error}"),
        );
    })?;
    let selected = &plan.downloads[selected_index];
    if let Some(capture) = &capture {
        capture
            .write_json(std::path::Path::new("plan.json"), &plan)
            .await?;
    }
    if selected.md5.is_empty() {
        progress.failed(
            OperationStage::Plan,
            "Selected Garmin deliverable has no MD5 integrity field",
        );
        bail!("selected Garmin deliverable has no MD5 integrity field");
    }
    let planning_elapsed = planning_started.elapsed();
    progress.completed_with_path(
        OperationStage::Plan,
        "Selected update file",
        selected.destination.as_path().display().to_string(),
        1,
        Some(1),
    );

    let probe_cache = cache_dir()?
        .join("pipeline-probes")
        .join(Uuid::new_v4().to_string());
    let result = run_transfers(
        &target,
        &device_identity,
        selected,
        &probe_cache,
        &progress,
        capture.clone(),
    )
    .await;
    let cleanup_result = match tokio::fs::remove_dir_all(&probe_cache).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    };

    let transfer = result?;
    let host_staging_removed =
        cleanup_result.context("unable to remove disposable host staging")?;
    let report = PipelineReport {
        device: manifest.summary,
        map_name: selected.map_name.clone(),
        destination: selected.destination.as_path().to_owned(),
        bytes: selected.size,
        inspect_seconds: inspect_elapsed.as_secs_f64(),
        query_seconds: query_elapsed.as_secs_f64(),
        planning_seconds: planning_elapsed.as_secs_f64(),
        download_seconds: transfer.download_elapsed.as_secs_f64(),
        download_bytes_per_second: bytes_per_second(selected.size, transfer.download_elapsed),
        md5_verified: true,
        device_probe: transfer.device_probe,
        total_seconds: total_started.elapsed().as_secs_f64(),
        host_staging_removed,
    };
    if let Some(capture) = &capture {
        capture
            .write_json(std::path::Path::new("benchmark-report.json"), &report)
            .await?;
    }
    Ok(report)
}

async fn inspect_device(
    target: &TargetArgs,
    progress: &ProgressReporter,
    capture: Option<&SessionCapture>,
) -> Result<(DeviceManifest, Duration)> {
    progress.started_with_path(
        OperationStage::Inspect,
        "Reading device manifest",
        "GarminDevice.xml",
        Some(1),
    );
    let started = Instant::now();
    let manifest = load_target(target).await.inspect_err(|error| {
        progress.failed(
            OperationStage::Inspect,
            format!("Device inspection failed: {error}"),
        );
    })?;
    capture_device_manifest(capture, &manifest).await?;
    progress.completed(
        OperationStage::Inspect,
        "Device manifest parsed",
        1,
        Some(1),
    );
    Ok((manifest, started.elapsed()))
}

fn select_probe_plan(
    response: &MapCatalog,
    device_digest: &str,
    filter: Option<&str>,
    service: MapService<'_>,
) -> Result<(garmin_update::UpdatePlan, usize)> {
    let map_count = response.maps.len() + response.bundled_maps.len();
    let mut selected = None;
    for map_index in 0..map_count {
        let plan = build_update_plan(response, device_digest.to_owned(), vec![map_index], service)?;
        let Some((download_index, download)) = matching_download(&plan.downloads, filter) else {
            continue;
        };
        let download_size = download.size;
        if selected
            .as_ref()
            .is_none_or(|(_, _, selected_size)| download_size > *selected_size)
        {
            selected = Some((plan, download_index, download_size));
        }
    }
    selected
        .map(|(plan, index, _)| (plan, index))
        .ok_or_else(|| match filter {
            Some(filter) => anyhow::anyhow!("no update download matched --map {filter:?}"),
            None => anyhow::anyhow!("Garmin reported no downloadable update files"),
        })
}

async fn capture_device_manifest(
    capture: Option<&SessionCapture>,
    manifest: &DeviceManifest,
) -> Result<()> {
    if let Some(capture) = capture {
        capture
            .write_bytes(
                std::path::Path::new("device/GarminDevice.xml"),
                manifest.raw_xml().as_bytes(),
            )
            .await?;
    }
    Ok(())
}

async fn query_updates(
    manifest: &DeviceManifest,
    progress: &ProgressReporter,
    mock_server: Option<&Url>,
    capture: Option<SessionCapture>,
) -> Result<(MapCatalog, Duration)> {
    let service_name = if mock_server.is_some() {
        "the local mock service"
    } else {
        "Garmin"
    };
    progress.started(
        OperationStage::Query,
        format!("Asking {service_name} for applicable map updates"),
        Some(1),
    );
    let started = Instant::now();
    let client = omt_client(MapService::from_mock_base(mock_server))?.with_capture(capture);
    let response = client
        .check_maps(
            manifest.raw_xml(),
            manifest.capabilities().installed_map_files(),
        )
        .await
        .inspect_err(|error| {
            progress.failed(
                OperationStage::Query,
                format!("Update query failed: {error}"),
            );
        })?;
    let elapsed = started.elapsed();
    progress.completed(
        OperationStage::Query,
        format!("Update metadata received from {service_name}"),
        1,
        Some(1),
    );
    Ok((response, elapsed))
}

struct TransferReport {
    download_elapsed: Duration,
    device_probe: DeviceProbeReport,
}

async fn run_transfers(
    target: &TargetArgs,
    device_identity: &str,
    selected: &DownloadSpec,
    probe_cache: &std::path::Path,
    progress: &ProgressReporter,
    capture: Option<SessionCapture>,
) -> Result<TransferReport> {
    let download_started = Instant::now();
    let staged = Downloader::new(1, GARMIN_EXPRESS_USER_AGENT)?
        .with_url_authorizer(Arc::new(GarminDownloadAuthorizer))
        .with_capture(capture)
        .stage_with_progress(selected, probe_cache, progress)
        .await?;
    let download_elapsed = download_started.elapsed();
    let recovery_root = cache_dir()?.join("link-probes").join("pending");
    let device_probe = probe_device_link(
        target,
        &staged.path,
        &recovery_root,
        device_identity,
        progress,
    )
    .await?;
    Ok(TransferReport {
        download_elapsed,
        device_probe,
    })
}

pub(crate) async fn probe_device_link(
    target: &TargetArgs,
    source: &std::path::Path,
    recovery_root: &std::path::Path,
    device_identity: &str,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport> {
    let device = device_link(target)?;
    let required = tokio::fs::metadata(source).await?.len();
    if let Some(capacity) = device.capacity().await? {
        progress.device_state(Ok(capacity.state.clone()));
        garmin_update::space::check_device_space(
            &capacity.state,
            &[StorageSpaceRequirement {
                storage_id: capacity.storage_id,
                required_free_bytes: required,
            }],
        )?;
    }
    let report = device
        .probe(
            &DeviceProbeRequest {
                source: source.to_owned(),
                recovery_root: recovery_root.to_owned(),
                device_identity: device_identity.to_owned(),
            },
            progress,
        )
        .await?;
    refresh_probe_state(device.as_ref(), progress).await;
    Ok(report)
}

pub(crate) async fn probe_raw_mtp_session(
    session: RawMtpSession,
    source: &std::path::Path,
    recovery_root: &std::path::Path,
    device_identity: &str,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport> {
    let required = tokio::fs::metadata(source).await?.len();
    let capacity = session.capacity().await?;
    progress.device_state(Ok(capacity.state.clone()));
    garmin_update::space::check_device_space(
        &capacity.state,
        &[StorageSpaceRequirement {
            storage_id: capacity.storage_id,
            required_free_bytes: required,
        }],
    )?;
    Ok(session
        .probe(
            &DeviceProbeRequest {
                source: source.to_owned(),
                recovery_root: recovery_root.to_owned(),
                device_identity: device_identity.to_owned(),
            },
            progress,
        )
        .await?)
}

fn device_link(target: &TargetArgs) -> Result<Box<dyn DeviceLink>> {
    match (
        target.path.as_ref(),
        target.mtp_location,
        target.mounted_mtp.as_deref(),
    ) {
        (Some(path), None, None) => Ok(Box::new(DirectoryDevice::new(path.clone()))),
        (None, Some(location), None) => Ok(Box::new(RawMtpLink::new(location))),
        (None, None, Some(mount_id)) => Ok(Box::new(MountedMtpDevice::new(mount_id))),
        _ => bail!("select exactly one device transport"),
    }
}

async fn refresh_probe_state(device: &dyn DeviceLink, progress: &ProgressReporter) {
    match device.capacity().await {
        Ok(Some(capacity)) => progress.device_state(Ok(capacity.state)),
        Ok(None) => {}
        Err(error) => progress.device_state(Err(error.to_string())),
    }
}

fn matching_download<'a>(
    downloads: &'a [DownloadSpec],
    filter: Option<&str>,
) -> Option<(usize, &'a DownloadSpec)> {
    let normalized_filter = filter.map(str::to_lowercase);
    downloads
        .iter()
        .enumerate()
        .filter(|download| {
            let download = download.1;
            normalized_filter.as_ref().is_none_or(|filter| {
                download.map_name.to_lowercase().contains(filter)
                    || download
                        .destination
                        .as_path()
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(filter)
            })
        })
        .max_by_key(|(_, download)| download.size)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "an approximate transfer rate is intentionally represented as f64"
)]
fn bytes_per_second(bytes: u64, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        0.0
    } else {
        bytes as f64 / elapsed.as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::{matching_download, select_probe_plan};
    use crate::MapService;
    use garmin_device::SafeRelativePath;
    use garmin_model::map::{MapCatalog, MapComponent, MapContent, MapDownload, MapInstallOption};
    use garmin_update::DownloadSpec;
    use url::Url;

    fn download(map_name: &str, destination: &str, size: u64) -> DownloadSpec {
        DownloadSpec {
            map_name: map_name.to_owned(),
            source: Url::parse("https://download.garmin.com/content.bin").unwrap(),
            alternate_sources: Vec::new(),
            requires_garmin_token: false,
            destination: SafeRelativePath::parse(destination).unwrap(),
            cache_name: "cache".to_owned(),
            size,
            md5: String::new(),
        }
    }

    #[test]
    fn selects_largest_download_by_default() {
        let downloads = [
            download("Small map", "Garmin/small.img", 10),
            download("Large map", "Garmin/large.img", 20),
        ];
        assert_eq!(matching_download(&downloads, None).unwrap().1.size, 20);
    }

    #[test]
    fn filters_by_map_or_destination_case_insensitively() {
        let downloads = [
            download("Example Regional Map", "Garmin/region.img", 10),
            download("Cycle map", "Garmin/TopoWest.img", 20),
        ];
        assert_eq!(
            matching_download(&downloads, Some("REGIONAL"))
                .unwrap()
                .1
                .map_name,
            "Example Regional Map"
        );
        assert_eq!(
            matching_download(&downloads, Some("topowest"))
                .unwrap()
                .1
                .map_name,
            "Cycle map"
        );
    }

    #[test]
    fn probe_selection_does_not_combine_unrelated_map_destinations() {
        fn map(name: &str) -> MapComponent {
            MapComponent {
                display_name: name.to_owned(),
                install_options: vec![MapInstallOption {
                    is_preferred: true,
                    files: vec![MapContent {
                        file_name: "Garmin/shared.img".to_owned(),
                        downloads: vec![MapDownload {
                            md5: "00".repeat(16),
                            size_in_bytes: 10,
                            delivery_type: Some("Full".to_owned()),
                            url: "https://download.garmin.com/shared.img".to_owned(),
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }
        }

        let response = MapCatalog {
            maps: vec![map("Base maps"), map("Another map")],
            ..Default::default()
        };
        let (plan, selected) =
            select_probe_plan(&response, "device", Some("Base maps"), MapService::Garmin).unwrap();

        assert_eq!(plan.downloads[selected].map_name, "Base maps");
    }
}
