//! Exercise delayed file-window completions through the desktop controller without hardware or a GPU.
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn desktop() -> Result<(tempfile::TempDir, Desktop), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let deployment = futures_lite::future::block_on(crate::mode::open(directory.path()))?;
    let application = futures_lite::future::block_on(deployment.application(deployment.epoch()))?;
    let users = futures_lite::future::block_on(application.profiles())?;
    drop(application);
    let mut desktop = Desktop::with_map_renderer(
        deployment,
        Translations::bundled()?,
        Context::default(),
        device_backend::open(directory.path())?,
        directory.path(),
        activity::map_runtime::Renderer::software(),
        profiling::RuntimeMetricsRecorder::default(),
    )?;
    desktop.profiles = users
        .into_iter()
        .map(|user| {
            ProfileView::new(
                worker::ProfileData {
                    user,
                    activities: Vec::new(),
                    avatar: None,
                },
                &desktop.intl,
                UnitSystem::Metric,
            )
        })
        .collect();
    desktop.selected_profile = Some(0);
    let key = garmin_fixtures::device::KEY;
    desktop.device_browser = Some(device_browser::Browser::open_directory(
        DeviceCatalogSnapshot {
            device_key: key.to_owned(),
            storages: vec![DeviceCatalogStorage {
                id: "internal".into(),
                label: "Synthetic storage".into(),
                entries: vec![
                    DeviceCatalogEntry {
                        path: "Garmin".into(),
                        kind: DeviceCatalogEntryKind::Directory,
                        size: None,
                    },
                    DeviceCatalogEntry {
                        path: "Garmin/Activity".into(),
                        kind: DeviceCatalogEntryKind::Directory,
                        size: None,
                    },
                ],
            }],
        },
        &desktop.intl,
        "Synthetic device",
        "internal",
        "Garmin/Activity",
    )?);
    desktop.file_window.open_inline(
        &desktop.context,
        WindowSpec {
            id: format!("device-files-{key}"),
            kind: "device-files".into(),
            title: "Synthetic files".into(),
            size: [640.0, 480.0],
        },
    );
    desktop.file_window_owner = Some(desktop.profiles[0].user.id());
    desktop.file_window_device = Some(key.to_owned());
    Ok((directory, desktop))
}

#[test]
fn refocusing_during_catalog_loading_preserves_the_directory() -> TestResult {
    let (_directory, mut desktop) = desktop()?;
    let key = garmin_fixtures::device::KEY;
    let before = serde_json::to_value(
        desktop
            .device_browser
            .as_ref()
            .expect("browser")
            .view_state(),
    )?;
    desktop.device_browser_loading = Some(key.to_owned());
    desktop.browse_device(key.to_owned());
    assert!(desktop.file_window.is_open());
    assert_eq!(desktop.device_browser_loading.as_deref(), Some(key));
    assert_eq!(
        serde_json::to_value(
            desktop
                .device_browser
                .as_ref()
                .expect("browser")
                .view_state()
        )?,
        before
    );
    Ok(())
}

#[test]
fn closing_a_busy_window_rejects_its_delayed_fit_preview() -> TestResult {
    let (_directory, mut desktop) = desktop()?;
    desktop.device_browser_status = DeviceBrowserStatus::Busy;
    desktop.handle_device_browser_action(&device_browser::Action::Close);
    assert!(!desktop.file_window.is_open());
    deliver_preview(&mut desktop);
    assert!(
        desktop.device_fit_preview.is_none(),
        "closed window must not publish a late preview"
    );
    assert!(
        !desktop.device_browser_status.is_busy(),
        "completion must release the operation"
    );
    Ok(())
}

#[test]
fn losing_the_selected_profile_rejects_a_delayed_fit_preview() -> TestResult {
    let (_directory, mut desktop) = desktop()?;
    desktop.device_browser_status = DeviceBrowserStatus::Busy;
    desktop.selected_profile = None;
    deliver_preview(&mut desktop);
    assert!(
        desktop.device_fit_preview.is_none(),
        "a preview must belong to the selected profile"
    );
    assert!(!desktop.device_browser_status.is_busy());
    Ok(())
}

#[test]
fn a_live_owned_window_receives_its_delayed_fit_preview() -> TestResult {
    let (_directory, mut desktop) = desktop()?;
    desktop.device_browser_status = DeviceBrowserStatus::Busy;
    deliver_preview(&mut desktop);
    assert!(desktop.device_fit_preview.is_some());
    assert!(!desktop.device_browser_status.is_busy());
    Ok(())
}

fn deliver_preview(desktop: &mut Desktop) {
    desktop.handle_device_browser_result(
        garmin_fixtures::device::KEY.to_owned(),
        worker::DeviceBrowserOperationKind::OpenFit,
        Ok(worker::DeviceBrowserOutcome::FitPreview {
            target: garmin_service_api::DeviceBrowserTarget {
                storage_id: "internal".into(),
                path: "Garmin/Activity/synthetic.fit".into(),
                kind: DeviceCatalogEntryKind::File,
            },
            preview: garmin_service_api::DeviceFitPreview {
                file_name: "synthetic.fit".into(),
                content: garmin_service_api::DeviceFitPreviewContent::Activities(Vec::new()),
            },
        }),
    );
}
