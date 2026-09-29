//! Actor authorization and portable activity snapshot round trips.

use std::{error::Error, fs::File};

use futures_lite::future::block_on;
use garmin_model::identity::{Role, UserId};
use garmin_services::{Application, UserContext, snapshots::Error as SnapshotError};
use garmin_storage::{Storage, snapshot::Limits};
use tempfile::tempdir;

#[test]
fn snapshots_preserve_activity_details_and_require_an_owner() -> Result<(), Box<dyn Error>> {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("source.sqlite3")).await?;
        garmin_fixtures::seed(&storage).await?;
        let mut application = Application::new(storage);
        let profiles = application.profiles().await?;
        let owner = profiles
            .iter()
            .find(|user| user.role() == Role::Owner)
            .ok_or("missing owner")?;
        let member = profiles
            .iter()
            .find(|user| user.role() == Role::Member)
            .ok_or("missing member")?;
        let output = root.path().join("snapshot.tar.zst");
        for actor in [
            UserContext::new(member.id()),
            UserContext::new(UserId::new_v4()),
        ] {
            assert!(matches!(
                application
                    .snapshot(actor, &output, Limits::default())
                    .await,
                Err(SnapshotError::OwnerRequired)
            ));
            assert!(matches!(
                application
                    .prepare_restore(actor, std::io::empty(), root.path(), Limits::default())
                    .await,
                Err(SnapshotError::OwnerRequired)
            ));
            assert!(!output.exists());
        }
        let actor = UserContext::new(owner.id());
        application
            .snapshot(actor, &output, Limits::default())
            .await?;
        let staged = application
            .prepare_restore(actor, File::open(output)?, root.path(), Limits::default())
            .await?;
        let destination = root.path().join("restored.sqlite3");
        staged.publish(&destination)?;
        let restored = Application::new(Storage::open(destination).await?);
        assert_eq!(restored.profiles().await?, profiles);
        for profile in profiles {
            let actor = UserContext::new(profile.id());
            let activities = application.activities(actor).await?;
            assert_eq!(restored.activities(actor).await?, activities);
            for activity in activities {
                assert_eq!(
                    restored.activity(actor, activity.observation_id()).await?,
                    application
                        .activity(actor, activity.observation_id())
                        .await?
                );
            }
        }
        restored.close().await;
        application.close().await;
        Ok(())
    })
}
