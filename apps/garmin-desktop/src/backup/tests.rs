use super::*;
use futures_lite::future::block_on;

fn wait(controller: &mut Controller, ready: impl Fn(&State) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !ready(&controller.state) {
        assert!(
            !matches!(controller.state, State::Failed(_)),
            "{:?}",
            controller.state
        );
        controller.state = controller
            .events
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("snapshot worker responds");
    }
}

#[test]
fn native_backup_confirmation_restore_and_restart() -> Result<(), Box<dyn std::error::Error>> {
    block_on(async {
        let root = tempfile::tempdir()?;
        let database = root.path().join("deployment");
        let deployment = Deployment::open(&database).await?;
        let epoch = deployment.epoch();
        let owner = deployment
            .application(epoch)
            .await?
            .create_profile("Owner".parse()?)
            .await?;
        let actor = UserContext::new(owner.id());
        let mut controller = Controller::new(
            Arc::clone(&deployment),
            egui::Context::default(),
            database.clone(),
        )?;
        let path = root.path().join("backup.tar.zst");
        controller.start(actor, epoch, Job::Backup(path.clone()));
        wait(&mut controller, |state| matches!(state, State::Saved));
        assert!(path.is_file());
        controller.clear();
        assert!(matches!(controller.state, State::Idle));
        assert!(controller.file.is_none());
        assert!(path.is_file());
        deployment
            .application(epoch)
            .await?
            .create_profile("After backup".parse()?)
            .await?;
        controller.start(actor, epoch, Job::Restore(path.clone()));
        wait(
            &mut controller,
            |state| matches!(state, State::Running(status) if status.preview.is_some()),
        );
        controller.clear();
        assert!(controller.state.busy());
        assert!(controller.file.is_some());
        assert_eq!(
            deployment.application(epoch).await?.profiles().await?.len(),
            2
        );
        controller.approve();
        wait(&mut controller, |state| matches!(state, State::Restored(_)));
        let State::Restored(summary) = &controller.state else {
            unreachable!("restore completed");
        };
        assert_eq!(summary.archive_bytes, Some(std::fs::metadata(&path)?.len()));
        assert!(summary.database_bytes.is_some_and(|bytes| bytes > 0));
        assert!(summary.elapsed.is_some());
        assert!(deployment.application(epoch).await.is_err());
        let restored = deployment.epoch();
        assert_eq!(
            deployment
                .application(restored)
                .await?
                .profiles()
                .await?
                .len(),
            1
        );
        drop(controller);
        deployment.close().await;
        drop(deployment);
        let reopened = Deployment::open(database).await?;
        assert_eq!(
            reopened
                .application(reopened.epoch())
                .await?
                .profiles()
                .await?
                .len(),
            1
        );
        reopened.close().await;
        Ok(())
    })
}

#[test]
fn cancelled_restore_and_invalid_input_leave_data_intact() -> Result<(), Box<dyn std::error::Error>>
{
    block_on(async {
        let root = tempfile::tempdir()?;
        let database = root.path().join("deployment");
        let deployment = Deployment::open(&database).await?;
        let epoch = deployment.epoch();
        let owner = deployment
            .application(epoch)
            .await?
            .create_profile("Owner".parse()?)
            .await?;
        let actor = UserContext::new(owner.id());
        let mut controller = Controller::new(
            Arc::clone(&deployment),
            egui::Context::default(),
            database.clone(),
        )?;
        let path = root.path().join("backup.tar.zst");
        controller.start(actor, epoch, Job::Backup(path.clone()));
        wait(&mut controller, |state| matches!(state, State::Saved));
        controller.start(actor, epoch, Job::Restore(path.clone()));
        wait(
            &mut controller,
            |state| matches!(state, State::Running(status) if status.preview.is_some()),
        );
        controller.cancel();
        wait(&mut controller, |state| matches!(state, State::Cancelled));
        assert_eq!(deployment.epoch(), epoch);
        assert_eq!(
            std::fs::read_dir(database.join(".tmp/snapshots"))?.count(),
            0
        );
        std::fs::write(&path, b"invalid snapshot")?;
        controller.start(actor, epoch, Job::Restore(path));
        wait(&mut controller, |state| matches!(state, State::Failed(_)));
        assert_eq!(
            deployment.application(epoch).await?.profiles().await?.len(),
            1
        );
        controller.start(actor, epoch, Job::Backup(database.join("storage.sqlite3")));
        assert!(matches!(controller.state, State::Failed(_)));
        drop(controller);
        deployment.close().await;
        Ok(())
    })
}

#[test]
fn member_and_failed_save_do_not_replace_destination() -> Result<(), Box<dyn std::error::Error>> {
    block_on(async {
        let root = tempfile::tempdir()?;
        let database = root.path().join("deployment");
        let deployment = Deployment::open(&database).await?;
        let epoch = deployment.epoch();
        let application = deployment.application(epoch).await?;
        let owner = application.create_profile("Owner".parse()?).await?;
        let member = application.create_profile("Member".parse()?).await?;
        drop(application);
        let destination = root.path().join("existing.tar.zst");
        std::fs::write(&destination, b"keep existing backup")?;
        let mut controller =
            Controller::new(Arc::clone(&deployment), egui::Context::default(), database)?;
        controller.start(
            UserContext::new(member.id()),
            epoch,
            Job::Backup(destination.clone()),
        );
        wait(&mut controller, |state| matches!(state, State::Failed(_)));
        assert_eq!(std::fs::read(&destination)?, b"keep existing backup");
        controller.start(
            UserContext::new(owner.id()),
            epoch,
            Job::Backup(destination.clone()),
        );
        controller.cancel();
        wait(&mut controller, |state| matches!(state, State::Cancelled));
        assert_eq!(std::fs::read(&destination)?, b"keep existing backup");
        let directory = root.path().join("directory.tar.zst");
        std::fs::create_dir(&directory)?;
        controller.start(
            UserContext::new(owner.id()),
            epoch,
            Job::Backup(directory.clone()),
        );
        wait(&mut controller, |state| matches!(state, State::Failed(_)));
        assert!(directory.is_dir());
        assert_eq!(std::fs::read_dir(root.path())?.count(), 3);
        drop(controller);
        deployment.close().await;
        Ok(())
    })
}
