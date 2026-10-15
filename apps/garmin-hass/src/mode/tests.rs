use std::path::Path;

use super::{DEPLOYMENT_MODE, DataError, PRODUCT_NAME, data_root, device_source, open};

#[test]
fn device_source_reports_a_missing_runtime() {
    let Err(DataError::DeviceSource(source)) = device_source(Path::new("unused")) else {
        panic!("a missing runtime must retain its typed source error");
    };
    assert!(
        source
            .downcast_ref::<tokio::runtime::TryCurrentError>()
            .is_some()
    );
}

#[tokio::test]
async fn startup_preserves_populated_storage_and_isolates_modes() -> anyhow::Result<()> {
    use garmin_service_api::DeploymentMode;

    let base = tempfile::tempdir()?;
    let root = data_root(base.path());
    let (expected_mode, expected_name, other_root) = if cfg!(feature = "demo") {
        (
            DeploymentMode::Demo,
            "Garmin Toolkit Demo",
            base.path().to_owned(),
        )
    } else {
        (
            DeploymentMode::Production,
            "Garmin Toolkit",
            base.path().join("demo"),
        )
    };
    assert_eq!(DEPLOYMENT_MODE, expected_mode);
    assert_eq!(PRODUCT_NAME, expected_name);
    assert_ne!(root, other_root);

    // Keep the other mode populated and locked while this mode starts and restarts.
    let other = garmin_services::deployment::Deployment::open(&other_root).await?;
    let sentinel = other
        .application(other.epoch())
        .await?
        .create_profile("Other deployment".parse()?)
        .await?;
    let deployment = open(&root).await?;
    let epoch = deployment.epoch();
    let mut expected = deployment.application(epoch).await?.profiles().await?;
    assert_eq!(expected.is_empty(), !cfg!(feature = "demo"));
    let profile = deployment
        .application(epoch)
        .await?
        .create_profile("Persisted profile".parse()?)
        .await?;
    expected.push(profile);
    expected.sort_by_key(garmin_model::identity::User::id);
    deployment.close().await;
    drop(deployment);

    let reopened = open(&root).await?;
    let mut actual = reopened
        .application(reopened.epoch())
        .await?
        .profiles()
        .await?;
    actual.sort_by_key(garmin_model::identity::User::id);
    assert_eq!(actual, expected);
    assert_eq!(
        other.application(other.epoch()).await?.profiles().await?,
        vec![sentinel]
    );
    reopened.close().await;
    other.close().await;
    Ok(())
}
