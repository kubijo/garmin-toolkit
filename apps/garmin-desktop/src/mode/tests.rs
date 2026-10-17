use super::{APP_ID, WINDOW_TITLE, open};

#[test]
fn application_identity_separates_platform_storage() {
    let (expected_id, expected_title, other_id) = if cfg!(feature = "demo") {
        (
            "io.kubijo.GarminToolkit.Demo",
            "Garmin Toolkit Demo",
            "io.kubijo.GarminToolkit",
        )
    } else {
        (
            "io.kubijo.GarminToolkit",
            "Garmin Toolkit",
            "io.kubijo.GarminToolkit.Demo",
        )
    };
    assert_eq!(APP_ID, expected_id);
    assert_eq!(WINDOW_TITLE, expected_title);
    let root = eframe::storage_dir(APP_ID).expect("platform application storage path");
    let other = eframe::storage_dir(other_id).expect("other mode's application storage path");
    assert_ne!(root, other);
}

#[test]
fn startup_preserves_populated_storage() -> Result<(), Box<dyn std::error::Error>> {
    futures_lite::future::block_on(async {
        let base = tempfile::tempdir()?;
        let root = base.path().join(APP_ID);
        let other_id = if cfg!(feature = "demo") {
            "io.kubijo.GarminToolkit"
        } else {
            "io.kubijo.GarminToolkit.Demo"
        };
        let other =
            garmin_services::deployment::Deployment::open(base.path().join(other_id)).await?;
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
    })
}
