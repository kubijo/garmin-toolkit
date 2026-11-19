use std::{error::Error, path::PathBuf};

use futures_lite::future::block_on;
use garmin_fit::fixture::ActivityCase;
use garmin_model::{
    artifact::AcquisitionOperationId,
    identity::{Source, SourceId},
    value::Timestamp,
};
use garmin_services::{Application, FitImportRequest, FitImportResult, UserContext};
use garmin_storage::Storage;

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("expected an output directory")?;
    std::fs::create_dir_all(&output)?;
    let fit = ActivityCase::CoastalRun;
    std::fs::write(output.join(fit.file_name()), fit.encode()?)?;

    block_on(async {
        let root = tempfile::tempdir()?;
        let application = Application::new(Storage::open(root.path().join("fixtures.db")).await?);
        let user = UserContext::new(
            application
                .create_profile("Fixture profile".parse()?)
                .await?
                .id(),
        );
        let source = Source::from_parts(
            SourceId::new_v4(),
            user.user_id(),
            "Development activities".parse()?,
            None,
        );
        let case = ActivityCase::IndoorPowerRide;
        let recording = case.encode()?;
        let FitImportResult::Imported { observations, .. } = application
            .import_fit(FitImportRequest::from_parts(
                user,
                &source,
                case.file_name().parse()?,
                AcquisitionOperationId::new_v4(),
                Timestamp::from_unix_milliseconds(case.end() * 1_000)?,
                &recording,
            ))
            .await?
        else {
            return Err("fixture recording was not imported".into());
        };
        let archive = output.join("indoor-power-ride.garmin-activity");
        application
            .export_activity(
                user,
                *observations.first().ok_or("fixture has no observations")?,
                &archive,
            )
            .await?
            .ok_or("fixture activity was not exported")?;
        application.preview_activity_package(user, &archive).await?;
        application
            .preview_activity_fit(user, &output.join(fit.file_name()))
            .await?;
        application.close().await;
        Ok(())
    })
}
