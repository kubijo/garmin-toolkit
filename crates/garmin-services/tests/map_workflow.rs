use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use garmin_device::{TransportKind, parse_manifest, storage::DirectoryDevice};
use garmin_service_api::maps::{
    Choice, Command, FailureKind, MapService, MapServiceServerShared, Phase, Request, State,
};
use garmin_services::maps::{
    CatalogSource, Operations, Session, Settings,
    device::{Connection, Connector},
    pending_recovery::PendingRecoveryStore,
};
use garmin_simulator::MockServer;
use remoc::rtc::ServerShared as _;
use uuid::Uuid;

struct Device(PathBuf);

#[async_trait]
impl Connector for Device {
    async fn connect(&self) -> Result<Connection> {
        let xml = tokio::fs::read_to_string(self.0.join("Garmin/GarminDevice.xml")).await?;
        Ok(Connection {
            manifest: parse_manifest(
                &xml,
                TransportKind::MassStorage,
                self.0.display().to_string(),
            )?,
            device: Box::new(DirectoryDevice::new(self.0.clone())),
        })
    }
}

fn settings(root: &Path, server: &MockServer) -> Settings {
    Settings {
        connector: Arc::new(Device(root.join("device"))),
        source: CatalogSource::Loopback(server.base_url().clone()),
        cache: root.join("cache"),
        captures: root.join("captures"),
        receipts: PendingRecoveryStore::new(root.join("receipts")),
        concurrency: 2,
        simulation_write_bytes_per_second: None,
    }
}

fn change(session: &Session, command: Command) -> Request {
    let request = Request::Change {
        request: Uuid::new_v4(),
        revision: session.snapshot().revision,
        command,
    };
    assert_wire_round_trip(&request);
    request
}

fn assert_wire_round_trip<
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
>(
    value: &T,
) {
    let bytes = postcard::to_stdvec(value).expect("portable wire model");
    let decoded: T = postcard::from_bytes(&bytes).expect("portable wire model");
    assert_eq!(&decoded, value);
}

async fn settled(session: &Session) -> State {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = session.snapshot();
            assert_wire_round_trip(&state);
            if !matches!(state.phase, Phase::Loading | Phase::Running) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("host job failed to settle")
}

#[tokio::test]
async fn local_and_rpc_share_revision_checks_and_retry_receipts() -> Result<()> {
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let operations = Operations::default();
    let session = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    assert_eq!(settled(&session).await.phase, Phase::Consent);
    assert!(
        !root.path().join("captures").exists(),
        "inspection must not contact the catalog"
    );
    let (rpc, client) =
        MapServiceServerShared::<_, remoc::codec::Default>::new(Arc::new(session.clone()));
    let serving = tokio::spawn(rpc.serve());
    let request = change(&session, Command::ContactService);
    let local = session.submit(request.clone()).unwrap();
    assert_eq!(client.request(request.clone()).await?.unwrap(), local);
    assert_eq!(settled(&session).await.phase, Phase::Catalog);
    assert_eq!(
        client.request(request.clone()).await?.unwrap(),
        local,
        "retry retains its original reply"
    );
    let mut reused = request;
    if let Request::Change { command, .. } = &mut reused {
        *command = Command::Refresh;
    }
    assert_eq!(
        session.submit(reused).unwrap_err().kind,
        FailureKind::ReusedRequest
    );
    let stale = change(
        &session,
        Command::Choose {
            component: 0,
            choice: Choice::Install,
        },
    );
    session
        .submit(change(&session, Command::VerifiedBackup(false)))
        .unwrap();
    assert_eq!(
        client.request(stale).await?.unwrap_err().kind,
        FailureKind::StaleRevision
    );
    let joined = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    assert_eq!(
        joined.snapshot(),
        session.snapshot(),
        "another profile/client joins the same workflow"
    );
    drop(client);
    serving.abort();
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn approved_job_survives_disconnect_and_excludes_browser_writes() -> Result<()> {
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let operations = Operations::default();
    let session = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    assert_eq!(settled(&session).await.phase, Phase::Catalog);
    session
        .submit(change(
            &session,
            Command::Choose {
                component: 0,
                choice: Choice::Install,
            },
        ))
        .unwrap();
    session.submit(change(&session, Command::Review)).unwrap();
    let review = settled(&session).await;
    assert_eq!(review.phase, Phase::Review, "{:?}", review.error);
    let approval = review.plan.unwrap().approval;
    let approval_request = change(&session, Command::Approve { approval });
    let accepted = session.submit(approval_request.clone()).unwrap();
    assert!(operations.mutations().acquire("fixture").is_err());
    let running = session.snapshot();
    let observer = session.subscribe();
    drop(observer);
    drop(session);
    let reconnected = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    tokio::time::sleep(Duration::from_millis(200)).await;
    if reconnected.snapshot().phase == Phase::Running {
        assert_eq!(
            reconnected.snapshot().revision,
            running.revision,
            "progress must not revise control state"
        );
    }
    let completed = settled(&reconnected).await;
    assert_eq!(completed.phase, Phase::Completed, "{:?}", completed.error);
    assert!(root.path().join("device/Garmin/Mock/europe.img").is_file());
    assert_eq!(completed.history.len(), 1);
    assert_eq!(
        reconnected.submit(approval_request).unwrap(),
        accepted,
        "a lost approval reply must not start a second mutation"
    );
    assert!(operations.mutations().acquire("fixture").is_ok());
    let restarted =
        Operations::default().connect("fixture".to_owned(), settings(root.path(), &server));
    let reopened = settled(&restarted).await;
    assert_eq!(reopened.phase, Phase::Consent);
    assert_eq!(
        reopened.history, completed.history,
        "completed outcomes survive a host restart"
    );
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn approval_cannot_be_reused_after_changing_backup_policy() -> Result<()> {
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let session =
        Operations::default().connect("fixture".to_owned(), settings(root.path(), &server));
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    settled(&session).await;
    session
        .submit(change(
            &session,
            Command::Choose {
                component: 0,
                choice: Choice::Install,
            },
        ))
        .unwrap();
    session.submit(change(&session, Command::Review)).unwrap();
    let original = settled(&session).await.plan.unwrap();
    session.submit(change(&session, Command::Back)).unwrap();
    session
        .submit(change(&session, Command::VerifiedBackup(false)))
        .unwrap();
    session.submit(change(&session, Command::Review)).unwrap();
    let replacement = settled(&session).await.plan.unwrap();
    assert_ne!(original.digest, replacement.digest);
    assert_ne!(original.approval, replacement.approval);
    let error = session
        .submit(change(
            &session,
            Command::Approve {
                approval: original.approval,
            },
        ))
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::InvalidChoice);
    assert!(!root.path().join("device/Garmin/Mock/europe.img").exists());
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn mixed_selection_requires_separate_approval_after_removal() -> Result<()> {
    let root = tempfile::tempdir()?;
    let device = garmin_fixtures::device::Device::open(root.path().join("device"))?;
    let server = MockServer::start().await?;
    let mut configuration = settings(root.path(), &server);
    configuration.connector = Arc::new(garmin_services::maps::device::DirectoryConnector {
        root: device.root().to_owned(),
        storage_id: garmin_fixtures::device::STORAGE_ID.to_owned(),
        storage_label: garmin_fixtures::device::STORAGE_LABEL.to_owned(),
    });
    let session = Operations::default().connect("fixture".to_owned(), configuration);
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    settled(&session).await;
    for (component, choice) in [(0, Choice::Remove), (1, Choice::Install)] {
        session
            .submit(change(&session, Command::Choose { component, choice }))
            .unwrap();
    }
    session.submit(change(&session, Command::Review)).unwrap();
    let removal = settled(&session).await;
    assert_eq!(removal.phase, Phase::Review, "{:?}", removal.error);
    let plan = removal.plan.unwrap();
    assert!(plan.removal);
    assert_eq!(
        plan.remove_count, 1,
        "the GUI fixture must contain an installed map"
    );
    session
        .submit(change(
            &session,
            Command::Approve {
                approval: plan.approval,
            },
        ))
        .unwrap();
    let update = settled(&session).await;
    assert_eq!(update.phase, Phase::Review, "{:?}", update.error);
    assert_eq!(update.history.len(), 1);
    assert!(
        !root
            .path()
            .join("device/Garmin/Mock/europe-old.img")
            .exists()
    );
    assert!(!root.path().join("device/Garmin/Mock/trails.img").exists());
    assert!(
        root.path()
            .join("device/Garmin/Mock/trails-old.img")
            .exists()
    );
    let plan = update.plan.unwrap();
    assert!(!plan.removal);
    session
        .submit(change(
            &session,
            Command::Approve {
                approval: plan.approval,
            },
        ))
        .unwrap();
    let completed = settled(&session).await;
    assert_eq!(completed.phase, Phase::Completed, "{:?}", completed.error);
    assert_eq!(completed.history.len(), 2);
    assert!(root.path().join("device/Garmin/Mock/trails.img").exists());
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn restart_requires_an_explicit_choice_for_retained_preparation() -> Result<()> {
    use garmin_capture::SessionCapture;
    use garmin_service_api::maps::Action;
    use garmin_services::maps::pending_recovery::PendingRecoveryKind;
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let configuration = settings(root.path(), &server);
    let connection = configuration.connector.connect().await?;
    let capture = SessionCapture::create(&root.path().join("retained"))?;
    configuration.receipts.register(
        PendingRecoveryKind::Update,
        capture.root(),
        &connection.manifest.identity_digest(),
        &"a".repeat(64),
    )?;
    let operations = Operations::default();
    let session = operations.connect("fixture".to_owned(), configuration);
    let state = settled(&session).await;
    assert_eq!(state.phase, Phase::Recovery);
    assert!(state.actions.contains(&Action::DiscardPreparation));
    assert!(!state.actions.contains(&Action::Recover));
    assert!(!state.actions.contains(&Action::Approve));
    assert!(!root.path().join("device/Garmin/Mock/europe.img").exists());
    session
        .submit(change(&session, Command::DiscardPreparation))
        .unwrap();
    assert_eq!(settled(&session).await.phase, Phase::Consent);
    assert!(
        capture.root().exists(),
        "discarding a notice must retain capture evidence"
    );
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn cancelling_catalog_loading_before_it_runs_does_not_contact_the_service() -> Result<()> {
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let session =
        Operations::default().connect("fixture".to_owned(), settings(root.path(), &server));
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    session.submit(change(&session, Command::Cancel)).unwrap();
    assert_eq!(settled(&session).await.phase, Phase::Cancelled);
    assert!(!root.path().join("captures").exists());
    assert!(session.snapshot().components.is_empty());
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn cancelling_an_approved_job_before_it_runs_preserves_the_device() -> Result<()> {
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let operations = Operations::default();
    let session = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    settled(&session).await;
    session
        .submit(change(
            &session,
            Command::Choose {
                component: 0,
                choice: Choice::Install,
            },
        ))
        .unwrap();
    session.submit(change(&session, Command::Review)).unwrap();
    let approval = settled(&session).await.plan.unwrap().approval;
    session
        .submit(change(&session, Command::Approve { approval }))
        .unwrap();
    session.submit(change(&session, Command::Cancel)).unwrap();
    let cancelled = settled(&session).await;
    assert_eq!(cancelled.phase, Phase::Cancelled, "{:?}", cancelled.error);
    assert!(
        root.path()
            .join("device/Garmin/Mock/europe-old.img")
            .is_file()
    );
    assert!(!root.path().join("device/Garmin/Mock/europe.img").exists());
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn retained_simulation_recovery_reopens_the_copy_and_never_mutates_its_source() -> Result<()>
{
    use garmin_services::maps::pending_recovery::PendingRecoveryKind;
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let server = MockServer::start().await?;
    let operations = Operations::default();
    let session = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    settled(&session).await;
    session
        .submit(change(
            &session,
            Command::Choose {
                component: 0,
                choice: Choice::Install,
            },
        ))
        .unwrap();
    session
        .submit(change(&session, Command::DryRun(true)))
        .unwrap();
    session.submit(change(&session, Command::Review)).unwrap();
    let approval = settled(&session).await.plan.unwrap().approval;
    session
        .submit(change(&session, Command::Approve { approval }))
        .unwrap();
    let completed = settled(&session).await;
    assert_eq!(completed.phase, Phase::Completed, "{:?}", completed.error);
    assert!(!root.path().join("device/Garmin/Mock/europe.img").exists());
    let capture = std::fs::read_dir(root.path().join("captures"))?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .find(|path| path.join("simulation/snapshot.json").exists())
        .unwrap();
    let configuration = settings(root.path(), &server);
    configuration
        .receipts
        .register_capture(PendingRecoveryKind::Update, &capture)?;
    let restarted = Operations::default().connect("fixture".to_owned(), configuration);
    let pending = settled(&restarted).await;
    assert_eq!(pending.phase, Phase::Recovery, "{:?}", pending.error);
    assert!(pending.recovery.unwrap().simulated);
    restarted
        .submit(change(&restarted, Command::Recover))
        .unwrap();
    let recovered = settled(&restarted).await;
    assert_eq!(recovered.phase, Phase::Consent, "{:?}", recovered.error);
    assert!(
        root.path()
            .join("device/Garmin/Mock/europe-old.img")
            .is_file()
    );
    assert!(!root.path().join("device/Garmin/Mock/europe.img").exists());
    server.shutdown().await?;
    Ok(())
}

const ASSISTED_WORKER_ROOT: &str = "GARMIN_MAP_ASSISTED_WORKER_ROOT";

#[tokio::test]
async fn assisted_recovery_crash_worker() -> Result<()> {
    let Some(root) = std::env::var_os(ASSISTED_WORKER_ROOT).map(PathBuf::from) else {
        return Ok(());
    };
    let server = MockServer::start().await?;
    let mut configuration = settings(&root, &server);
    configuration.simulation_write_bytes_per_second = std::num::NonZeroU64::new(1_000_000);
    let session = Operations::default().connect("fixture".to_owned(), configuration);
    settled(&session).await;
    session
        .submit(change(&session, Command::ContactService))
        .unwrap();
    settled(&session).await;
    session
        .submit(change(
            &session,
            Command::Choose {
                component: 0,
                choice: Choice::Install,
            },
        ))
        .unwrap();
    session
        .submit(change(&session, Command::DryRun(true)))
        .unwrap();
    session.submit(change(&session, Command::Review)).unwrap();
    let approval = settled(&session).await.plan.unwrap().approval;
    session
        .submit(change(&session, Command::Approve { approval }))
        .unwrap();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn interrupted_simulation(root: &Path) -> Result<PathBuf> {
    struct Worker(std::process::Child);
    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut worker = Worker(
        std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "assisted_recovery_crash_worker", "--nocapture"])
            .env(ASSISTED_WORKER_ROOT, root)
            .spawn()?,
    );
    let capture = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(entries) = std::fs::read_dir(root.join("captures")) {
                for entry in entries {
                    let capture = entry?.path();
                    if !capture
                        .join("mounted-update/transaction/000000-prepared.json")
                        .exists()
                    {
                        continue;
                    }
                    let events = std::fs::read_to_string(capture.join("events.jsonl"))?;
                    let writing = events
                        .lines()
                        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                        .any(|event| {
                            event["stage"] == "commit"
                                && event["path"] == "Garmin/Mock/europe.img"
                                && event["completed"]
                                    .as_u64()
                                    .is_some_and(|bytes| bytes >= 100_000)
                        });
                    if writing {
                        return Ok::<_, anyhow::Error>(capture);
                    }
                }
            }
            if let Some(status) = worker.0.try_wait()? {
                anyhow::bail!("simulation worker exited before interruption: {status}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    worker.0.kill()?;
    worker.0.wait()?;
    Ok(capture)
}

#[tokio::test]
async fn assisted_recovery_requires_current_approval_and_survives_client_reconnect() -> Result<()> {
    use garmin_service_api::maps::Action;
    let root = tempfile::tempdir()?;
    garmin_simulator::create_fixture(&root.path().join("device")).await?;
    let capture = interrupted_simulation(root.path()).await?;
    let server = MockServer::start().await?;
    let operations = Operations::default();
    let session = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    assert_eq!(settled(&session).await.phase, Phase::Recovery);
    assert_eq!(
        session
            .submit(change(
                &session,
                Command::ApproveRecovery {
                    approval: Uuid::new_v4()
                }
            ))
            .unwrap_err()
            .kind,
        FailureKind::Unavailable
    );
    session.submit(change(&session, Command::Recover)).unwrap();
    let blocked = settled(&session).await;
    assert_eq!(blocked.phase, Phase::Recovery);
    assert!(
        blocked.error.is_some(),
        "ordinary recovery must still refuse the empty upload"
    );
    session
        .submit(change(&session, Command::ReviewRecovery))
        .unwrap();
    let reviewed = settled(&session).await;
    assert!(reviewed.error.is_none(), "{:?}", reviewed.error);
    assert!(!reviewed.actions.contains(&Action::Recover));
    let original = reviewed.recovery.unwrap().review.unwrap();
    assert_eq!(original.files.len(), 1);
    assert_eq!(original.files[0].path, "Garmin/Mock/europe.img");
    session.submit(change(&session, Command::Back)).unwrap();
    session
        .submit(change(&session, Command::ReviewRecovery))
        .unwrap();
    let review = settled(&session).await.recovery.unwrap().review.unwrap();
    assert_ne!(original.approval, review.approval);
    assert_eq!(
        session
            .submit(change(
                &session,
                Command::ApproveRecovery {
                    approval: original.approval
                }
            ))
            .unwrap_err()
            .kind,
        FailureKind::InvalidChoice
    );
    let reconnected = operations.connect("fixture".to_owned(), settings(root.path(), &server));
    assert_eq!(
        reconnected.snapshot().recovery.unwrap().review.unwrap(),
        review
    );
    let request = change(
        &reconnected,
        Command::ApproveRecovery {
            approval: review.approval,
        },
    );
    let reply = reconnected.submit(request.clone()).unwrap();
    assert_eq!(reconnected.submit(request.clone()).unwrap(), reply);
    let recovered = settled(&reconnected).await;
    assert_eq!(recovered.phase, Phase::Consent, "{:?}", recovered.error);
    assert!(recovered.error.is_none());
    assert_eq!(
        recovered
            .history
            .iter()
            .filter(|outcome| outcome.recovered)
            .count(),
        1
    );
    assert_eq!(reconnected.submit(request).unwrap(), reply);
    verify_assisted_files(root.path(), &capture, &recovered)?;
    let restarted =
        Operations::default().connect("fixture".to_owned(), settings(root.path(), &server));
    let persisted = settled(&restarted).await;
    assert_eq!(persisted.phase, Phase::Consent);
    assert_eq!(persisted.history, recovered.history);
    server.shutdown().await?;
    Ok(())
}

fn verify_assisted_files(root: &Path, capture: &Path, recovered: &State) -> Result<()> {
    let quarantines = std::fs::read_dir(capture)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("assisted-recovery-")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        quarantines.len(),
        1,
        "retry must not execute recovery twice"
    );
    assert_eq!(
        std::fs::metadata(quarantines[0].path().join("000000.bin"))?.len(),
        0
    );
    assert!(recovered.history.iter().any(|outcome| {
        outcome.recovered
            && outcome
                .message
                .contains(quarantines[0].path().to_str().unwrap())
    }));
    for device in [
        root.join("device"),
        capture.join("simulation/device/storage-001"),
    ] {
        assert_eq!(
            std::fs::read(device.join("Garmin/Mock/europe-old.img"))?,
            b"old mock content\n"
        );
        assert!(!device.join("Garmin/Mock/europe.img").exists());
    }
    Ok(())
}
