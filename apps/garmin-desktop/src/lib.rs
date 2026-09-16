//! Native desktop composition.

#![recursion_limit = "256"]
#![expect(
    clippy::multiple_crate_versions,
    reason = "eframe and device/rendering adapters require incompatible transitive releases"
)]

use std::fs;

use futures_lite::future::block_on;
use garmin_i18n::Translations;
use garmin_services::Application;
use thiserror::Error;

const MULTISAMPLING: u16 = 4;

mod device_backend;
mod map_worker;
mod mode;
mod profiling;
mod view;
mod window;
mod worker;

pub use mode::DataError;

/// Opens the platform database and native window.
///
/// The `demo` build uses a separate seeded database and directory-backed fake device.
/// # Errors
/// [`enum@Error`] when application data or the native window cannot be initialized.
pub fn run() -> Result<(), Error> {
    let profiling = profiling::RuntimeMetricsRecorder::from_environment()?;
    let data_root = eframe::storage_dir(mode::APP_ID).ok_or(Error::DataRootUnavailable)?;
    fs::create_dir_all(&data_root)?;
    let storage = block_on(mode::open_storage(mode::database_path(&data_root)))?;
    let device_platform = device_backend::open(&data_root)?;
    let application = Application::new(storage);
    let translations = Translations::bundled()?;
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_app_id(mode::APP_ID)
        .with_icon(garmin_ui::brand::icon())
        .with_decorations(false)
        .with_inner_size([1_100.0, 720.0])
        .with_min_inner_size([720.0, 480.0]);

    let map_metrics = profiling.clone();
    let window_result = eframe::run_native(
        mode::WINDOW_TITLE,
        eframe::NativeOptions {
            viewport,
            multisampling: MULTISAMPLING,
            ..Default::default()
        },
        Box::new(move |creation| {
            garmin_ui::install(&creation.egui_ctx);
            let map_renderer = creation
                .wgpu_render_state
                .as_ref()
                .ok_or(Error::MapRendererUnavailable)
                .map(|render_state| {
                    garmin_ui::activity::install_wgpu_map(render_state, u32::from(MULTISAMPLING))
                })?;
            Ok(Box::new(view::Desktop::new(
                application,
                translations,
                creation.egui_ctx.clone(),
                device_platform,
                &data_root,
                map_renderer,
                map_metrics,
            )?))
        }),
    );
    let profiling_result = profiling.finish();
    window_result?;
    profiling_result?;
    Ok(())
}

/// Desktop startup failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("the platform application-data directory is unavailable")]
    DataRootUnavailable,
    #[error("eframe did not provide the required WGPU render state")]
    MapRendererUnavailable,
    #[error("could not prepare the application-data directory: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error(transparent)]
    Device(#[from] device_backend::Error),
    #[error(transparent)]
    Application(#[from] garmin_services::Error),
    #[error(transparent)]
    Localization(#[from] garmin_i18n::Error),
    #[error(transparent)]
    Profiling(#[from] profiling::Error),
    #[error(transparent)]
    Window(#[from] eframe::Error),
}
