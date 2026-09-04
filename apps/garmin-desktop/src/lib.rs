//! Native desktop composition.

#![expect(
    clippy::multiple_crate_versions,
    reason = "eframe and device/rendering adapters require incompatible transitive releases"
)]

use std::fs;

use futures_lite::future::block_on;
use garmin_i18n::Translations;
use garmin_services::Application;
use thiserror::Error;

mod device_backend;
mod mode;
mod view;
mod window;
mod worker;

pub use mode::DataError;

/// Opens the platform database and native window.
///
/// The `demo` build uses a separate data root seeded through the production importer.
/// # Errors
/// [`enum@Error`] when application data or the native window cannot be initialized.
pub fn run() -> Result<(), Error> {
    let data_root = eframe::storage_dir(mode::APP_ID).ok_or(Error::DataRootUnavailable)?;
    fs::create_dir_all(&data_root)?;
    let storage = block_on(mode::open_storage(mode::database_path(&data_root)))?;
    let application = Application::new(storage);
    let translations = Translations::bundled()?;
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_app_id(mode::APP_ID)
        .with_icon(garmin_ui::brand::icon())
        .with_decorations(false)
        .with_inner_size([1_100.0, 720.0])
        .with_min_inner_size([720.0, 480.0]);

    eframe::run_native(
        mode::WINDOW_TITLE,
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |creation| {
            garmin_ui::install(&creation.egui_ctx);
            Ok(Box::new(view::Desktop::new(
                application,
                translations,
                creation.egui_ctx.clone(),
            )?))
        }),
    )?;
    Ok(())
}

/// Desktop startup failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("the platform application-data directory is unavailable")]
    DataRootUnavailable,
    #[error("could not prepare the application-data directory: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error(transparent)]
    Application(#[from] garmin_services::Error),
    #[error(transparent)]
    Localization(#[from] garmin_i18n::Error),
    #[error(transparent)]
    Window(#[from] eframe::Error),
}
