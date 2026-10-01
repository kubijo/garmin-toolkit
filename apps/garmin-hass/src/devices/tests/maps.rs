//! Connection lifetime and browser-write exclusion through the real host service.

use std::{path::Path, time::Duration};

use garmin_service_api::{
    ApplicationService as _, DeviceBrowserRequest, DeviceBrowserUpload, DeviceCatalogEntryKind,
    maps::{Choice, Command, MapService as _, Phase, Request, State},
};

use super::{
    DEMO_DEVICE_KEY, DEMO_STORAGE_ID, Host, demo_browser, demo_source, demo_upload, target,
};

async fn phase(watch: &mut remoc::rch::watch::Receiver<State>, expected: Phase) -> State {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = watch.borrow_and_update().unwrap().clone();
            if state.phase == expected {
                return state;
            }
            assert!(state.error.is_none(), "{:?}", state.error);
            watch.changed().await.unwrap();
        }
    })
    .await
    .expect("map workflow did not reach the expected phase")
}

fn change(state: &State, command: Command) -> Request {
    Request::Change {
        request: uuid::Uuid::new_v4(),
        revision: state.revision,
        command,
    }
}

fn create_folder() -> DeviceBrowserRequest {
    DeviceBrowserRequest::CreateDirectory {
        device_key: DEMO_DEVICE_KEY.into(),
        storage_id: DEMO_STORAGE_ID.into(),
        parent: "Garmin".into(),
        name: "AfterMaps".into(),
    }
}

async fn start_then_disconnect(host: &Host, dry_run: bool) -> (Request, State) {
    let connection = host.with_control(None);
    let client = connection
        .maps(DEMO_DEVICE_KEY.into())
        .await
        .unwrap()
        .unwrap();
    let mut watch = client.watch().await.unwrap();
    let consent = phase(&mut watch, Phase::Consent).await;
    client
        .request(change(&consent, Command::ContactService))
        .await
        .unwrap()
        .unwrap();
    let catalog = phase(&mut watch, Phase::Catalog).await;
    let chosen = client
        .request(change(
            &catalog,
            Command::Choose {
                component: 0,
                choice: Choice::Install,
            },
        ))
        .await
        .unwrap()
        .unwrap();
    let configured = client
        .request(change(&chosen, Command::DryRun(dry_run)))
        .await
        .unwrap()
        .unwrap();
    client
        .request(change(&configured, Command::Review))
        .await
        .unwrap()
        .unwrap();
    let review = phase(&mut watch, Phase::Review).await;
    let approval = change(
        &review,
        Command::Approve {
            approval: review.plan.as_ref().unwrap().approval,
        },
    );
    // Discard the acknowledgement, then drop the watch, client, and connection on return.
    client.request(approval.clone()).await.unwrap().unwrap();
    (approval, phase(&mut watch, Phase::Running).await)
}

async fn assert_browser_writes_blocked(host: &Host, device: &Path) {
    let busy = "another operation is modifying this device";
    assert_eq!(demo_browser(host, create_folder()).await.unwrap_err(), busy);
    assert_eq!(
        demo_browser(
            host,
            DeviceBrowserRequest::Remove {
                device_key: DEMO_DEVICE_KEY.into(),
                target: target("Garmin/Mock/trails-old.img", DeviceCatalogEntryKind::File),
            }
        )
        .await
        .unwrap_err(),
        busy
    );
    let (_sender, receiver) = remoc::rch::io::sized::<remoc::codec::Default>(1);
    let rejected = tokio::time::timeout(
        Duration::from_secs(1),
        host.upload_device_browser_file(
            DeviceBrowserUpload {
                device_key: DEMO_DEVICE_KEY.into(),
                storage_id: DEMO_STORAGE_ID.into(),
                directory: "Garmin".into(),
                file_name: "blocked.bin".into(),
                size: 1,
            },
            receiver,
        ),
    )
    .await
    .expect("busy upload must reject before waiting for its body")
    .unwrap();
    assert_eq!(rejected.unwrap_err(), busy);
    assert!(!device.join("Garmin/AfterMaps").exists());
    assert!(!device.join("Garmin/blocked.bin").exists());
    assert_eq!(
        std::fs::read(device.join("Garmin/Mock/trails-old.img")).unwrap(),
        b"old mock content\n"
    );
}

#[tokio::test]
async fn reconnect_retries_approval_and_excludes_browser_writes() {
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::prepare_deployment(directory.path()).await.unwrap();
    let host = Host::new(demo_source(&directory), storage);
    let device = directory.path().join("device");
    let (approval, running) = start_then_disconnect(&host, false).await;
    assert_browser_writes_blocked(&host, &device).await;

    let connection = host.with_control(None);
    let reconnected = connection
        .maps(DEMO_DEVICE_KEY.into())
        .await
        .unwrap()
        .unwrap();
    let mut watch = reconnected.watch().await.unwrap();
    let retry = reconnected
        .request(approval.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.id, running.id);
    assert_eq!(retry.phase, Phase::Running);
    let completed = phase(&mut watch, Phase::Completed).await;
    assert_eq!(completed.id, running.id);
    assert_eq!(
        completed.history.len(),
        1,
        "approval retry must not start another job"
    );
    assert!(device.join("Garmin/Mock/europe.img").is_file());
    assert_eq!(reconnected.request(approval).await.unwrap().unwrap(), retry);
    demo_browser(&host, create_folder()).await.unwrap();
    demo_upload(
        &host,
        "Garmin/AfterMaps",
        "allowed.bin",
        b"after completion",
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(device.join("Garmin/AfterMaps/allowed.bin")).unwrap(),
        b"after completion"
    );
}

async fn partial_write(watch: &mut remoc::rch::watch::Receiver<State>, after: u64) -> State {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = watch.borrow_and_update().unwrap().clone();
            assert_eq!(
                state.phase,
                Phase::Running,
                "write should still be active: {state:?}"
            );
            if written(&state) > after {
                return state;
            }
            watch.changed().await.unwrap();
        }
    })
    .await
    .expect("paced upload must make progress")
}

fn written(state: &State) -> u64 {
    state
        .progress
        .iter()
        .filter(|progress| progress.stage == "Commit" && progress.bytes)
        .map(|progress| progress.completed)
        .max()
        .unwrap_or_default()
}

#[tokio::test]
async fn paced_simulation_continues_progress_after_reconnect() {
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::prepare_deployment(directory.path()).await.unwrap();
    let host = Host::with_simulation_write_rate(
        demo_source(&directory),
        storage,
        std::num::NonZeroU64::new(11_000_000),
    );
    let (approval, running) = start_then_disconnect(&host, true).await;
    let before_disconnect = {
        let connection = host.with_control(None);
        let client = connection
            .maps(DEMO_DEVICE_KEY.into())
            .await
            .unwrap()
            .unwrap();
        let mut watch = client.watch().await.unwrap();
        partial_write(&mut watch, 0).await
    };
    // No client or progress watch exists while the host advances the upload.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let connection = host.with_control(None);
    let client = connection
        .maps(DEMO_DEVICE_KEY.into())
        .await
        .unwrap()
        .unwrap();
    let mut watch = client.watch().await.unwrap();
    let after_reconnect = partial_write(&mut watch, written(&before_disconnect)).await;
    assert_eq!(after_reconnect.id, running.id);
    assert!(after_reconnect.dry_run);
    let completed = phase(&mut watch, Phase::Completed).await;
    assert_eq!(completed.id, running.id);
    assert_eq!(completed.history.len(), 1);
    client.request(approval).await.unwrap().unwrap();
    let state = client.request(Request::State).await.unwrap().unwrap();
    assert_eq!(
        state.history.len(),
        1,
        "approval replay must not create a second operation"
    );
    let device = directory.path().join("device/Garmin/Mock");
    assert!(!device.join("europe.img").exists());
    assert_eq!(
        std::fs::read(device.join("europe-old.img")).unwrap(),
        b"old mock content\n"
    );
}
