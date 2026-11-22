use std::{path::Path, sync::Arc, time::Duration};

use garmin_capture::SessionCapture;
use garmin_device::{TransportKind, parse_manifest};
use garmin_fixtures::firmware::{EDGE_32_20, FENIX_24_20, FENIX_GSP_22_44};
use garmin_model::artifact::ByteCount;
use garmin_progress::ProgressReporter;
use garmin_service_api::firmware::{FirmwareService as _, Phase, State};
use garmin_services::{
    firmware::Session,
    maps::{Operations, device::DirectoryConnector, pending_recovery::PendingRecoveryStore},
};
use garmin_update::firmware::{Confirmation, FirmwareReceipt};

const OLD: &str = FENIX_GSP_22_44.inventory;
const CURRENT: &str = FENIX_24_20.inventory;
const EDGE: &str = EDGE_32_20.inventory;

async fn session(host: &Operations, root: &Path) -> Session {
    host.open_firmware(
        "fixture".into(),
        Arc::new(DirectoryConnector {
            root: root.to_owned(),
            storage_id: "internal".into(),
            storage_label: "Internal".into(),
        }),
        true,
    )
    .await
    .unwrap()
}

async fn lookup(session: &Session) -> State {
    let revision = session.snapshot().revision;
    assert_eq!(
        session.start_check(revision).unwrap().phase,
        Phase::Checking
    );
    assert!(session.start_check(revision).is_err());
    wait_for_catalog(session).await
}

async fn wait_for_catalog(session: &Session) -> State {
    let mut watch = session.watch().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = watch.borrow_and_update().unwrap().clone();
            if matches!(state.phase, Phase::Available(_) | Phase::Failed(_)) {
                return state;
            }
            watch.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn reconnecting_the_same_key_checks_the_new_inventory() {
    let directory = tempfile::tempdir().unwrap();
    let inventory = directory.path().join("GarminDevice.xml");
    std::fs::write(&inventory, OLD).unwrap();
    let host = Operations::default();
    let old = session(&host, directory.path()).await;
    old.check_on_attachment();
    let _catalog = wait_for_catalog(&old).await;
    assert_eq!(old.available_update().unwrap().version, "24.20");
    let revision = old.snapshot().revision;
    old.check_on_attachment();
    assert_eq!(old.snapshot().revision, revision);

    host.forget_firmware("fixture");
    assert_eq!(old.snapshot().phase, Phase::Disconnected);
    assert!(old.start_check(old.snapshot().revision).is_err());
    assert!(old.available_update().is_none());
    std::fs::write(&inventory, CURRENT).unwrap();
    let current = session(&host, directory.path()).await;
    current.check_on_attachment();
    let state = wait_for_catalog(&current).await;
    assert!(matches!(state.phase, Phase::Available(ref offers) if !offers.is_empty()));
    assert!(current.available_update().is_none());
    assert!(
        host.firmware_session("fixture")
            .unwrap()
            .available_update()
            .is_none()
    );
}

#[tokio::test]
async fn recovery_waiting_for_a_connection_loses_approval_on_detach() {
    use garmin_service_api::firmware::Action;
    use garmin_services::deployment::Deployment;
    use garmin_services::maps::{
        device::{Connection, Connector},
        pending_recovery::PendingRecoveryKind,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    struct PausedConnector {
        directory: DirectoryConnector,
        pause: AtomicBool,
        waiting: Notify,
        resume: Notify,
    }
    #[async_trait::async_trait]
    impl Connector for PausedConnector {
        async fn connect(&self) -> anyhow::Result<Connection> {
            if self.pause.swap(false, Ordering::AcqRel) {
                self.waiting.notify_one();
                self.resume.notified().await;
            }
            self.directory.connect().await
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("device");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("GarminDevice.xml"), OLD).unwrap();
    let host_root = temp.path().join("host");
    let deployment = Deployment::open(&host_root).await.unwrap();
    let registry = PendingRecoveryStore::new(host_root.join("firmware/receipts"));
    let operations = Operations::new(deployment);
    let connector = Arc::new(PausedConnector {
        directory: DirectoryConnector {
            root,
            storage_id: "internal".into(),
            storage_label: "Internal".into(),
        },
        waiting: Notify::new(),
        resume: Notify::new(),
        pause: AtomicBool::new(false),
    });
    let available_device = connector.connect().await.unwrap();
    let manifest = &available_device.manifest;
    let digest = manifest.identity_digest();
    let capture = SessionCapture::create(&temp.path().join("capture")).unwrap();
    let catalog =
        garmin_update::firmware::FirmwareCatalog::from_json(FENIX_GSP_22_44.catalog.as_bytes())
            .unwrap();
    let receipt = FirmwareReceipt::new(manifest, "plan", &catalog.updates).unwrap();
    capture
        .write_json_atomic(Path::new("firmware/selection.json"), &receipt)
        .await
        .unwrap();
    registry
        .register(PendingRecoveryKind::Update, capture.root(), &digest, "plan")
        .unwrap();
    let old = operations
        .open_firmware("watch".into(), connector.clone(), true)
        .await
        .unwrap();
    let mut watch = old.watch().await.unwrap();
    old.submit(old.snapshot().revision, Action::Inspect)
        .unwrap();
    let inspected = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = watch.borrow_and_update().unwrap().clone();
            match state.phase {
                Phase::RecoveryRequired => break state,
                Phase::Checking => watch.changed().await.unwrap(),
                phase => panic!("unexpected phase {phase:?}"),
            }
        }
    })
    .await
    .unwrap();
    connector.pause.store(true, Ordering::Release);
    old.submit(inspected.revision, Action::Recover).unwrap();
    tokio::time::timeout(Duration::from_secs(5), connector.waiting.notified())
        .await
        .unwrap();
    operations.forget_firmware("watch");
    let new = operations
        .open_firmware("watch".into(), connector.clone(), true)
        .await
        .unwrap();
    connector.resume.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while operations.mutations().acquire("watch").is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(old.snapshot().phase, Phase::Disconnected);
    assert_eq!(new.snapshot().phase, Phase::Consent);
    assert_eq!(registry.for_device(&digest).unwrap().len(), 1);
}

#[tokio::test]
async fn watchers_share_a_review_bound_to_the_device_and_both_deliverables() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("GarminDevice.xml");
    std::fs::write(&manifest, OLD).unwrap();
    let host = Operations::default();
    let original = session(&host, directory.path()).await;
    assert_eq!(original.snapshot().phase, Phase::Consent);
    let available = lookup(&original).await;
    let Phase::Available(ref components) = available.phase else {
        panic!("{available:?}");
    };
    assert_eq!(components.len(), 3);
    assert!(components[0].system_bundle);
    assert!(components[0].has_metadata);
    assert_eq!(components[0].installed.as_deref(), Some("22.44"));
    assert_eq!(components[0].available, "24.20");
    let size = ByteCount::from_bytes(12 + 117);
    assert_eq!(components[0].download_bytes, Some(size));
    let json = serde_json::to_string(&available).unwrap();
    assert!(!json.contains("127.0.0.1") && !json.contains("downloadUrl"));
    let other = session(&host, directory.path()).await;
    assert!(
        other
            .review_selection(available.revision, vec![1, 1])
            .is_err()
    );
    let review = other.review_selection(available.revision, vec![0]).unwrap();
    assert_eq!(review, original.snapshot());
    let approval = review.review.unwrap();
    assert_eq!(approval.download_bytes, size);
    let lock = host.mutations().acquire("fixture").unwrap();
    assert!(
        original
            .claim_for_staging(review.revision, approval.approval)
            .await
            .is_err()
    );
    drop(lock);
    let claimed = original
        .claim_for_staging(review.revision, approval.approval)
        .await
        .unwrap();
    assert_eq!(claimed.offers().len(), 2);
    assert_eq!(claimed.offers()[1].part_number, "006-V4532-00-META");
    let targets = claimed.targets();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].storage_id, "internal");
    assert_eq!(targets[0].path.to_string(), "Garmin/GUPDATE.GSP");
    assert_eq!(
        targets[1].path.to_string(),
        "Garmin/UpdateMetadata/006v453200.json"
    );
    assert_eq!(targets[1].offered_bytes, ByteCount::from_bytes(117));
    assert!(targets.iter().all(|t| t.existing_bytes.is_none()));
    let cache = tempfile::tempdir().unwrap();
    let (first, second) = tokio::join!(
        claimed.download(cache.path(), ProgressReporter::default()),
        claimed.download(cache.path(), ProgressReporter::default()),
    );
    let files = first.unwrap();
    for (a, b) in files.iter().zip(second.unwrap()) {
        assert_ne!(a.from_cache, b.from_cache);
        assert_eq!(a.path, b.path);
    }
    assert_eq!(std::fs::read(&files[0].path).unwrap(), b"006-V4532-00");
    assert_eq!(files[1].bytes, ByteCount::from_bytes(117));
    assert!(
        claimed
            .download(cache.path(), ProgressReporter::default())
            .await
            .unwrap()
            .iter()
            .all(|f| f.from_cache)
    );
    assert!(host.mutations().acquire("fixture").is_err());
    drop(claimed);
    assert!(
        other
            .claim_for_staging(review.revision, approval.approval)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(manifest).unwrap(), OLD);
}

#[tokio::test]
async fn demo_watch_offers_a_stageable_system_bundle_without_changing_its_fixture() {
    let directory = tempfile::tempdir().unwrap();
    let xml = garmin_fixtures::device::MANIFEST_XML;
    std::fs::write(directory.path().join("GarminDevice.xml"), xml).unwrap();
    let host = Operations::default();
    let session = session(&host, directory.path()).await;
    let (_, components) = session.check_components().await.unwrap();
    assert_eq!(components.len(), 3);
    assert!(
        components
            .iter()
            .any(|item| item.system_bundle && item.has_metadata && item.download_bytes.is_some())
    );
}

#[tokio::test]
async fn stale_versions_and_replaced_devices_invalidate_the_review() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("GarminDevice.xml");
    let host = Operations::default();
    let session = session(&host, directory.path()).await;
    for replacement in [CURRENT, EDGE] {
        std::fs::write(&path, OLD).unwrap();
        let available = lookup(&session).await;
        let reviewed = session
            .review_selection(available.revision, vec![0])
            .unwrap();
        let approval = reviewed.review.unwrap().approval;
        std::fs::write(&path, replacement).unwrap();
        assert!(
            session
                .claim_for_staging(reviewed.revision, approval)
                .await
                .is_err()
        );
        std::fs::write(&path, OLD).unwrap();
        assert!(
            session
                .claim_for_staging(reviewed.revision, approval)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn stages_the_verified_watch_bundle_and_metadata_with_a_recovery_journal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    let garmin = root.join("Garmin");
    std::fs::create_dir_all(&garmin).unwrap();
    let manifest = garmin.join("GarminDevice.xml");
    std::fs::write(&manifest, OLD).unwrap();
    let bundle = garmin.join("GUPDATE.GSP");
    std::fs::write(&bundle, b"previous staged bundle").unwrap();
    let host = Operations::default();
    let session = session(&host, &root).await;
    let available = lookup(&session).await;
    let review = session
        .review_selection(available.revision, vec![0, 1])
        .unwrap();
    let approved = session
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("firmware-recoveries"));
    let report = approved
        .stage(
            &directory.path().join("cache"),
            &capture,
            &registry,
            ProgressReporter::default(),
        )
        .await
        .unwrap();
    assert_eq!(report.files_written, 4);
    assert_eq!(std::fs::read(bundle).unwrap(), b"006-V4532-00");
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(garmin.join("UpdateMetadata/006v453200.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["sourceVersion"], "22.44");
    assert_eq!(metadata["destinationVersion"], "24.20");
    assert!(
        capture
            .root()
            .join("mounted-update/transaction/committed.json")
            .is_file()
    );
    assert_eq!(std::fs::read_to_string(manifest).unwrap(), OLD);
    let receipt: FirmwareReceipt = serde_json::from_slice(
        &std::fs::read(capture.root().join("firmware/selection.json")).unwrap(),
    )
    .unwrap();
    let read = |xml| parse_manifest(xml, TransportKind::MassStorage, "fixture".into()).unwrap();
    let pending = registry.for_device(&read(OLD).identity_digest()).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].transaction, capture.root());
    assert!(pending[0].is_prepared().unwrap());
    assert_eq!(
        receipt
            .confirm(&read(CURRENT))
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|item| (item.part_number, item.status))
            .collect::<Vec<_>>(),
        [
            ("006-V4532-00", Confirmation::Confirmed),
            ("006-DB775-00", Confirmation::Unchanged)
        ]
    );
    assert!(receipt.confirm(&read(EDGE)).unwrap().is_none());
}

async fn interrupt_bundle_and_video_commit(
    root: &Path,
    cache: &Path,
    capture: &SessionCapture,
    registry: &PendingRecoveryStore,
) {
    let host = Operations::default();
    let session = session(&host, root).await;
    let available = lookup(&session).await;
    let review = session
        .review_selection(available.revision, vec![0, 1])
        .unwrap();
    let approved = session
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let inject_failure = root.join("GARMIN-TOOLKIT/transactions");
    let progress = ProgressReporter::from_handler(move |_| {
        if let Ok(entries) = std::fs::read_dir(&inject_failure) {
            for entry in entries {
                std::fs::create_dir_all(entry.unwrap().path().join("completed.json")).unwrap();
            }
        }
    });
    assert!(
        approved
            .stage(cache, capture, registry, progress)
            .await
            .is_err()
    );
    assert!(
        capture
            .root()
            .join("mounted-update/transaction/committed.json")
            .is_file()
    );
}

#[tokio::test]
async fn missing_capture_and_interrupted_commit_retain_the_firmware_receipt() {
    use garmin_services::firmware::{TransferStatus, recover_registered, status};
    use garmin_services::maps::device::Connector as _;
    use garmin_update::MountedUpdateRecoveryOutcome;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir_all(root.join("Garmin")).unwrap();
    std::fs::write(root.join("Garmin/GarminDevice.xml"), OLD).unwrap();
    let connector = DirectoryConnector {
        root: root.clone(),
        storage_id: "internal".into(),
        storage_label: "Internal".into(),
    };
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("receipts"));
    interrupt_bundle_and_video_commit(&root, &directory.path().join("cache"), &capture, &registry)
        .await;
    let device_state = root.join("GARMIN-TOOLKIT");
    let transactions = device_state.join("transactions");
    let device = connector.connect().await.unwrap();
    let digest = device.manifest.identity_digest();
    assert!(matches!(
        status(&device, &registry, false).await.unwrap().as_slice(),
        [TransferStatus::RecoveryRequired]
    ));
    assert!(status(&device, &registry, true).await.is_err());

    let moved = directory.path().join("offline-capture");
    std::fs::rename(capture.root(), &moved).unwrap();
    let progress = ProgressReporter::default();
    let error = recover_registered(&device, &registry, None, &progress)
        .await
        .unwrap_err();
    assert!(error.to_string().contains(capture.root().to_str().unwrap()));
    assert_eq!(registry.for_device(&digest).unwrap().len(), 1);
    assert!(device_state.join("active.json").is_file());
    std::fs::rename(moved, capture.root()).unwrap();
    let transaction = std::fs::read_dir(transactions)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_dir(transaction.join("completed.json")).unwrap();
    std::fs::remove_file(root.join("Garmin/GUPDATE.GSP")).unwrap();
    std::fs::remove_file(root.join("Garmin/UpdateMetadata/006v453200.json")).unwrap();
    assert!(
        recover_registered(&device, &registry, None, &progress)
            .await
            .is_err()
    );
    assert!(device_state.join("active.json").is_file());
    std::fs::write(root.join("Garmin/GarminDevice.xml"), CURRENT).unwrap();
    let device = connector.connect().await.unwrap();
    // The second component still reports its old version, so neither of its files may disappear.
    let video = root.join("Garmin/RemoteSW/006-DB775-00.zvp");
    let metadata = root.join("Garmin/UpdateMetadata/006db77500.json");
    for file in [video, metadata] {
        let saved = std::fs::read(&file).unwrap();
        std::fs::remove_file(&file).unwrap();
        assert!(
            recover_registered(&device, &registry, None, &progress)
                .await
                .is_err()
        );
        assert!(device_state.join("active.json").is_file());
        std::fs::write(file, saved).unwrap();
    }
    let recovered = recover_registered(&device, &registry, None, &progress)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.outcome, MountedUpdateRecoveryOutcome::Committed);
    assert!(!device_state.join("active.json").exists());
    assert_eq!(registry.for_device(&digest).unwrap().len(), 1);
    assert!(matches!(
        status(&device, &registry, true).await.unwrap().as_slice(),
        [TransferStatus::AwaitingConfirmation { components }] if components.iter().map(|c| &c.status).eq([&Confirmation::Confirmed, &Confirmation::Unchanged])
    ));
    assert!(registry.for_device(&digest).unwrap().is_empty());
}

#[tokio::test]
async fn detaching_during_download_invalidates_a_claimed_transfer() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir_all(root.join("Garmin")).unwrap();
    std::fs::write(root.join("Garmin/GarminDevice.xml"), OLD).unwrap();
    let host = Arc::new(Operations::default());
    let original = session(&host, &root).await;
    let available = lookup(&original).await;
    let review = original
        .review_selection(available.revision, vec![0])
        .unwrap();
    let approved = original
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let detach = Arc::clone(&host);
    let progress = ProgressReporter::from_handler(move |_| detach.forget_firmware("fixture"));
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("receipts"));
    assert!(
        approved
            .stage(
                &directory.path().join("cache"),
                &capture,
                &registry,
                progress
            )
            .await
            .is_err()
    );
    assert_eq!(original.snapshot().phase, Phase::Disconnected);
    assert!(!root.join("Garmin/GUPDATE.GSP").exists());
    assert!(!root.join("GARMIN-TOOLKIT").exists());
    let new = session(&host, &root).await;
    assert!(matches!(lookup(&new).await.phase, Phase::Available(_)));
}

#[tokio::test]
async fn detaching_after_the_first_write_requires_fresh_recovery() {
    use garmin_progress::{OperationStage, ProgressScope, ProgressState};
    use garmin_services::maps::device::Connector as _;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("device");
    let garmin = root.join("Garmin");
    std::fs::create_dir_all(&garmin).unwrap();
    std::fs::write(garmin.join("GarminDevice.xml"), OLD).unwrap();
    let host = Arc::new(Operations::default());
    let session = session(&host, &root).await;
    let catalog = lookup(&session).await;
    let review = session.review_selection(catalog.revision, vec![0]).unwrap();
    let approved = session
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let capture = SessionCapture::create(&temp.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(temp.path().join("receipts"));
    let progress = ProgressReporter::from_handler(move |event| {
        if event.scope == ProgressScope::Stage
            && event.stage == OperationStage::Commit
            && event.state == ProgressState::Advanced
            && event.path.as_deref() == Some("Garmin/GUPDATE.GSP")
        {
            host.forget_firmware("fixture");
        }
    });
    assert!(
        approved
            .stage(&temp.path().join("cache"), &capture, &registry, progress)
            .await
            .is_err()
    );
    assert_eq!(session.snapshot().phase, Phase::Disconnected);
    assert!(garmin.join("GUPDATE.GSP").is_file());
    assert!(root.join("GARMIN-TOOLKIT/active.json").is_file());
    let device = DirectoryConnector {
        root,
        storage_id: "internal".into(),
        storage_label: "Internal".into(),
    }
    .connect()
    .await
    .unwrap();
    let result = garmin_services::firmware::recover_registered(
        &device,
        &registry,
        None,
        &ProgressReporter::default(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        result.outcome,
        garmin_update::MountedUpdateRecoveryOutcome::Restored
    );
    assert!(!garmin.join("GUPDATE.GSP").exists());
}

#[tokio::test]
async fn guided_firmware_writes_both_files_only_to_the_virtual_device() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("fixture");
    std::fs::create_dir_all(root.join("Garmin")).unwrap();
    std::fs::write(root.join("Garmin/GarminDevice.xml"), OLD).unwrap();
    std::fs::write(root.join("Garmin/GUPDATE.GSP"), b"original").unwrap();
    let host = Operations::default();
    let session = session(&host, &root).await;
    let available = lookup(&session).await;
    let review = session
        .review_selection(available.revision, vec![0])
        .unwrap();
    let approved = session
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("virtual-receipts"));
    approved
        .on_virtual_device()
        .stage(
            &directory.path().join("cache"),
            &capture,
            &registry,
            ProgressReporter::default(),
        )
        .await
        .unwrap();
    let shadow = capture.root().join("simulation/device/storage-001/Garmin");
    assert_eq!(
        std::fs::read(shadow.join("GUPDATE.GSP")).unwrap(),
        b"006-V4532-00"
    );
    assert!(shadow.join("UpdateMetadata/006v453200.json").is_file());
    assert_eq!(
        std::fs::read(root.join("Garmin/GUPDATE.GSP")).unwrap(),
        b"original"
    );
    assert!(!root.join("GARMIN-TOOLKIT").exists());
    assert!(!root.join("Garmin/UpdateMetadata").exists());
}

#[tokio::test]
async fn interrupted_empty_firmware_upload_requires_review_before_rollback() {
    use garmin_services::firmware::{
        approve_empty_uploads, recover_registered, review_empty_uploads,
    };
    use garmin_services::maps::device::Connector as _;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    let garmin = root.join("Garmin");
    std::fs::create_dir_all(&garmin).unwrap();
    std::fs::write(garmin.join("GarminDevice.xml"), OLD).unwrap();
    let connector = || DirectoryConnector {
        root: root.clone(),
        storage_id: "internal".into(),
        storage_label: "Internal".into(),
    };
    let host = Operations::default();
    let session = host
        .open_firmware(
            "fixture".into(),
            Arc::new(connector().with_transfer_delays(Duration::from_millis(200), Duration::ZERO)),
            true,
        )
        .await
        .unwrap();
    let available = lookup(&session).await;
    let review = session
        .review_selection(available.revision, vec![0])
        .unwrap();
    let approved = session
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let cache = directory.path().join("cache");
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("receipts"));
    let staging_registry = registry.clone();
    let task = tokio::spawn(async move {
        approved
            .stage(
                &cache,
                &capture,
                &staging_registry,
                ProgressReporter::default(),
            )
            .await
    });
    let bundle = garmin.join("GUPDATE.GSP");
    tokio::time::timeout(Duration::from_secs(15), async {
        while !bundle.is_file() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let connection = connector().connect().await.unwrap();
    let progress = ProgressReporter::default();
    assert!(
        recover_registered(&connection, &registry, None, &progress)
            .await
            .is_err()
    );
    let reviewed = review_empty_uploads(&connection, &registry, None, &progress)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reviewed.files()[0].path.to_string(), "Garmin/GUPDATE.GSP");
    assert_eq!(std::fs::metadata(&bundle).unwrap().len(), 0);
    approve_empty_uploads(&connection, &registry, None, &reviewed, &progress)
        .await
        .unwrap();
    assert!(!bundle.exists());
    assert!(
        registry
            .for_device(&connection.manifest.identity_digest())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn existing_receipts_move_to_the_shared_registry_even_with_an_offline_capture() {
    use garmin_services::maps::pending_recovery::PendingRecoveryKind;
    let directory = tempfile::tempdir().unwrap();
    let capture = directory.path().join("removable-drive");
    std::fs::create_dir(&capture).unwrap();
    let old = PendingRecoveryStore::new(directory.path().join("desktop"));
    let shared = PendingRecoveryStore::new(directory.path().join("shared"));
    let digest = "a".repeat(32);
    let plan = "b".repeat(64);
    old.register(PendingRecoveryKind::Update, &capture, &digest, &plan)
        .unwrap();
    std::fs::rename(&capture, directory.path().join("unmounted")).unwrap();
    shared.adopt_firmware_receipts(&old).unwrap();
    shared.adopt_firmware_receipts(&old).unwrap();
    let receipts = shared.for_device(&digest).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].transaction, capture);
    assert!(old.for_device(&digest).unwrap().is_empty());
}

async fn receipt_action(session: &Session, action: garmin_service_api::firmware::Action) -> Phase {
    let mut watch = session.watch().await.unwrap();
    session.submit(session.snapshot().revision, action).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let phase = watch.borrow_and_update().unwrap().phase.clone();
            if !matches!(
                phase,
                Phase::Checking | Phase::Recovering | Phase::Staging(_)
            ) {
                return phase;
            }
            watch.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn completed_graphical_transfer_keeps_stage_rows_and_file_history() {
    use garmin_service_api::firmware::{Action, OperationStage, ProgressState, WRITE_STAGES};
    use garmin_services::deployment::Deployment;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("GarminDevice.xml"), OLD).unwrap();
    let host = Operations::new(Deployment::open(directory.path()).await.unwrap());
    let session = session(&host, &root).await;
    let catalog = lookup(&session).await;
    let reviewed = session.review_selection(catalog.revision, vec![0]).unwrap();
    let phase = receipt_action(&session, Action::Stage(reviewed.review.unwrap().approval)).await;
    assert!(matches!(phase, Phase::ReadyToEject { .. }), "{phase:?}");
    let transcript = session.snapshot().transcript;
    assert!(
        transcript
            .stages
            .iter()
            .map(|row| row.stage)
            .eq(WRITE_STAGES.iter().copied())
    );
    assert!(
        transcript
            .stages
            .iter()
            .any(|row| row.stage == OperationStage::Commit
                && row.state == Some(ProgressState::Completed))
    );
    assert!(
        transcript
            .history
            .iter()
            .any(|event| event.path.is_some() && event.state == Some(ProgressState::Completed))
    );
}

#[tokio::test]
async fn unprepared_cleanup_retains_a_receipt_with_a_different_selection() {
    use garmin_services::firmware::recover_registered;
    use garmin_services::maps::{device::Connector as _, pending_recovery::PendingRecoveryKind};
    use garmin_update::firmware::FirmwareCatalog;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("GarminDevice.xml"), OLD).unwrap();
    let connection = DirectoryConnector {
        root,
        storage_id: "internal".into(),
        storage_label: "Internal".into(),
    }
    .connect()
    .await
    .unwrap();
    let catalog = FirmwareCatalog::from_json(FENIX_GSP_22_44.catalog.as_bytes()).unwrap();
    let receipt =
        FirmwareReceipt::new(&connection.manifest, "previous-plan", &catalog.updates).unwrap();
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    capture
        .write_json_atomic(Path::new("firmware/selection.json"), &receipt)
        .await
        .unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("receipts"));
    let digest = connection.manifest.identity_digest();
    registry
        .register(
            PendingRecoveryKind::Update,
            capture.root(),
            &digest,
            "selected-plan",
        )
        .unwrap();
    let error = recover_registered(&connection, &registry, None, &ProgressReporter::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("selection does not match"));
    assert_eq!(registry.for_device(&digest).unwrap().len(), 1);
}

#[tokio::test]
async fn graphical_dismissal_requires_the_reviewed_registration_and_valid_journals() {
    use garmin_service_api::firmware::{Action, ReceiptStatus};
    use garmin_services::deployment::Deployment;
    use garmin_services::maps::pending_recovery::PendingRecoveryKind;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("GarminDevice.xml"), OLD).unwrap();
    let host = Operations::new(Deployment::open(directory.path()).await.unwrap());
    let session = session(&host, &root).await;
    let review = review_edited_selection(&session).await;
    let approved = session
        .claim_for_staging(review.revision, review.review.unwrap().approval)
        .await
        .unwrap();
    let digest = approved.connection().manifest.identity_digest();
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let shared = PendingRecoveryStore::new(directory.path().join("firmware/receipts"));
    approved
        .stage(
            &directory.path().join("cache"),
            &capture,
            &shared,
            ProgressReporter::default(),
        )
        .await
        .unwrap();
    let approval = inspect_and_close_over_reviewed_catalog(&session).await;
    assert_eq!(shared.for_device(&digest).unwrap().len(), 1);
    assert!(matches!(
        receipt_action(&session, Action::Stage(approval)).await,
        Phase::PreviousTransferPending
    ));
    assert!(matches!(
        receipt_action(&session, Action::Inspect).await,
        Phase::Reconnected(_)
    ));
    let first = shared.for_device(&digest).unwrap().pop().unwrap();
    shared.clear(&first).unwrap();
    let current = shared
        .register(first.kind, &first.transaction, &digest, &first.plan_digest)
        .unwrap();
    let id = current.id().unwrap();
    assert_ne!(id, first.id().unwrap());
    assert!(matches!(
        receipt_action(&session, Action::Acknowledge).await,
        Phase::Failed(_)
    ));
    assert_eq!(shared.for_device(&digest).unwrap()[0].id().unwrap(), id);

    let journal = capture
        .root()
        .join("mounted-update/transaction/committed.json");
    let contents = std::fs::read(&journal).unwrap();
    assert!(matches!(
        receipt_action(&session, Action::Inspect).await,
        Phase::Reconnected(_)
    ));
    std::fs::write(&journal, []).unwrap();
    assert!(matches!(
        receipt_action(&session, Action::Acknowledge).await,
        Phase::Failed(_)
    ));
    assert_eq!(shared.for_device(&digest).unwrap()[0].id().unwrap(), id);
    std::fs::write(&journal, contents).unwrap();

    let legacy = PendingRecoveryStore::new(directory.path().join("legacy"));
    let missing = directory.path().join("removable");
    std::fs::create_dir(&missing).unwrap();
    legacy
        .register(PendingRecoveryKind::Update, &missing, &digest, "older-plan")
        .unwrap();
    std::fs::remove_dir(&missing).unwrap();
    shared.adopt_firmware_receipts(&legacy).unwrap();
    let Phase::Transfers(choices) = receipt_action(&session, Action::Inspect).await else {
        panic!("expected both retained transfers");
    };
    assert_eq!(choices.len(), 2);
    assert!(
        choices
            .iter()
            .any(|r| r.receipt == id && r.status == ReceiptStatus::AwaitingConfirmation)
    );
    assert!(
        choices
            .iter()
            .any(|r| r.receipt != id && r.status == ReceiptStatus::RecoveryRequired)
    );
    assert!(matches!(
        receipt_action(&session, Action::InspectReceipt(id)).await,
        Phase::Reconnected(_)
    ));
    assert_eq!(
        receipt_action(&session, Action::Acknowledge).await,
        Phase::RecoveryRequired
    );
    let remaining = shared.for_device(&digest).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].transaction, missing);
}

async fn review_edited_selection(session: &Session) -> State {
    use garmin_service_api::firmware::Action;

    let available = lookup(session).await;
    assert!(!available.has_retained_transfer);
    let first_review = session
        .review_selection(available.revision, vec![0])
        .unwrap();
    let edited = session
        .submit(first_review.revision, Action::EditSelection)
        .unwrap();
    assert!(edited.review.is_none());
    assert!(
        session
            .submit(
                edited.revision,
                Action::Stage(first_review.review.unwrap().approval)
            )
            .is_err()
    );
    session.review_selection(edited.revision, vec![0]).unwrap()
}

async fn inspect_and_close_over_reviewed_catalog(session: &Session) -> uuid::Uuid {
    use garmin_service_api::firmware::Action;
    let available = lookup(session).await;
    assert!(available.has_retained_transfer);
    let review = session
        .review_selection(available.revision, vec![0])
        .unwrap();
    assert!(matches!(
        receipt_action(session, Action::Inspect).await,
        Phase::Reconnected(_)
    ));
    let inspected = session.snapshot();
    assert_eq!(
        inspected.inspection.as_ref().unwrap().background,
        available.phase
    );
    let closed = session
        .submit(inspected.revision, Action::CloseInspection)
        .unwrap();
    assert_eq!(closed.phase, available.phase);
    assert_eq!(closed.review, review.review);
    assert_eq!(closed.catalog_id, available.catalog_id);
    closed.review.unwrap().approval
}

#[tokio::test]
async fn recovery_matches_the_active_plan_among_migrated_receipts() {
    use garmin_services::firmware::recover_registered;
    use garmin_services::maps::{device::Connector as _, pending_recovery::PendingRecoveryKind};
    use garmin_update::MountedUpdateRecoveryOutcome;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir_all(root.join("Garmin")).unwrap();
    std::fs::write(root.join("Garmin/GarminDevice.xml"), OLD).unwrap();
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let shared = PendingRecoveryStore::new(directory.path().join("receipts"));
    interrupt_bundle_and_video_commit(&root, &directory.path().join("cache"), &capture, &shared)
        .await;
    let connection = DirectoryConnector {
        root: root.clone(),
        storage_label: "Internal".into(),
        storage_id: "internal".into(),
    }
    .connect()
    .await
    .unwrap();
    let digest = connection.manifest.identity_digest();
    let legacy = PendingRecoveryStore::new(directory.path().join("legacy"));
    let other = directory.path().join("other");
    std::fs::create_dir(&other).unwrap();
    legacy
        .register(
            PendingRecoveryKind::Update,
            &other,
            &digest,
            "previous-plan",
        )
        .unwrap();
    shared.adopt_firmware_receipts(&legacy).unwrap();
    let entries = shared.for_device(&digest).unwrap();
    assert_eq!(entries.len(), 2);
    let unrelated = entries
        .iter()
        .find(|p| p.transaction == other)
        .unwrap()
        .id()
        .unwrap();
    let progress = ProgressReporter::default();
    assert!(
        recover_registered(&connection, &shared, Some(unrelated), &progress)
            .await
            .is_err()
    );
    let transactions = root.join("GARMIN-TOOLKIT/transactions");
    let active = std::fs::read_dir(transactions)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_dir(active.join("completed.json")).unwrap();
    let recovered = recover_registered(&connection, &shared, None, &progress)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.outcome, MountedUpdateRecoveryOutcome::Committed);
    assert!(!root.join("GARMIN-TOOLKIT/active.json").exists());
    assert_eq!(shared.for_device(&digest).unwrap().len(), 2);
}

fn rewrite_preparation(capture: &Path, edit: impl Fn(&mut serde_json::Value)) {
    for name in ["000000-prepared.json", "committed.json"] {
        let path = capture.join("mounted-update/transaction").join(name);
        let mut journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        edit(&mut journal);
        std::fs::write(path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn same_plan_distinguishes_capture_identity_original_hashes_and_legacy_history() {
    use garmin_services::firmware::{
        TransferStatus, recover_registered, recovery_receipt_id, review_status,
    };
    use garmin_services::maps::{device::Connector as _, pending_recovery::PendingRecoveryKind};
    use serde_json::Value;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("device");
    std::fs::create_dir_all(root.join("Garmin")).unwrap();
    std::fs::write(root.join("Garmin/GarminDevice.xml"), OLD).unwrap();
    std::fs::write(root.join("Garmin/GUPDATE.GSP"), b"original bundle").unwrap();
    let capture = SessionCapture::create(&temp.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(temp.path().join("receipts"));
    interrupt_bundle_and_video_commit(&root, &temp.path().join("cache"), &capture, &registry).await;
    let connection = DirectoryConnector {
        root: root.clone(),
        storage_label: "Internal".into(),
        storage_id: "internal".into(),
    }
    .connect()
    .await
    .unwrap();
    let digest = connection.manifest.identity_digest();
    let original = registry.for_device(&digest).unwrap().pop().unwrap();
    let id = original.id().unwrap();

    let other = SessionCapture::create(&temp.path().join("prior")).unwrap();
    copy_firmware_journals(&capture, &other);
    let prior = registry
        .register(
            PendingRecoveryKind::Update,
            other.root(),
            &digest,
            &original.plan_digest,
        )
        .unwrap()
        .id()
        .unwrap();
    let prior_id = uuid::Uuid::new_v4();
    rewrite_preparation(other.root(), |journal| {
        journal["transaction_id"] = Value::from(prior_id.to_string());
    });
    assert_eq!(
        recovery_receipt_id(&connection, &registry, None)
            .await
            .unwrap(),
        id
    );
    assert!(
        recovery_receipt_id(&connection, &registry, Some(prior))
            .await
            .is_err()
    );
    let reports = review_status(&connection, &registry, Some(prior), |_, _| Ok(false))
        .await
        .unwrap();
    assert!(matches!(
        reports[0].status,
        TransferStatus::AwaitingConfirmation { .. }
    ));

    let prior = check_mixed_captures(
        &connection,
        &registry,
        &other,
        &original.plan_digest,
        id,
        prior,
    )
    .await;
    check_legacy_captures(&connection, &registry, &capture, &other, id, prior).await;
    let active = std::fs::read_dir(root.join("GARMIN-TOOLKIT/transactions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_dir(active.join("completed.json")).unwrap();
    recover_registered(
        &connection,
        &registry,
        Some(prior),
        &ProgressReporter::default(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!root.join("GARMIN-TOOLKIT/active.json").exists());
}

fn copy_firmware_journals(capture: &SessionCapture, other: &SessionCapture) {
    let journals = Path::new("mounted-update/transaction");
    std::fs::create_dir_all(other.root().join(journals)).unwrap();
    for entry in std::fs::read_dir(capture.root().join(journals)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            other.root().join(journals).join(entry.file_name()),
        )
        .unwrap();
    }
    std::fs::create_dir(other.root().join("firmware")).unwrap();
    let selection = Path::new("firmware/selection.json");
    std::fs::copy(capture.root().join(selection), other.root().join(selection)).unwrap();
}

async fn check_mixed_captures(
    connection: &garmin_services::maps::device::Connection,
    registry: &PendingRecoveryStore,
    legacy: &SessionCapture,
    plan: &str,
    active: uuid::Uuid,
    previous: uuid::Uuid,
) -> uuid::Uuid {
    use garmin_services::firmware::{TransferStatus, recovery_receipt_id, review_status};
    use garmin_services::maps::pending_recovery::PendingRecoveryKind;
    rewrite_preparation(legacy.root(), |journal| {
        journal.as_object_mut().unwrap().remove("transaction_id");
    });
    let reports = review_status(connection, registry, None, |_, _| Ok(false))
        .await
        .unwrap();
    assert!(
        reports
            .iter()
            .any(|r| r.receipt == active && matches!(r.status, TransferStatus::RecoveryRequired))
    );
    assert!(reports.iter().any(|r| r.receipt == previous
        && matches!(r.status, TransferStatus::AwaitingConfirmation { .. })));
    assert!(
        recovery_receipt_id(connection, registry, Some(previous))
            .await
            .is_err()
    );
    review_status(connection, registry, Some(previous), |report, _| {
        assert!(matches!(
            report.status,
            TransferStatus::AwaitingConfirmation { .. }
        ));
        Ok(true)
    })
    .await
    .unwrap();
    let identity = connection.manifest.identity_digest();
    assert_eq!(registry.for_device(&identity).unwrap().len(), 1);
    assert_eq!(
        recovery_receipt_id(connection, registry, None)
            .await
            .unwrap(),
        active
    );
    registry
        .register(PendingRecoveryKind::Update, legacy.root(), &identity, plan)
        .unwrap()
        .id()
        .unwrap()
}

async fn check_legacy_captures(
    connection: &garmin_services::maps::device::Connection,
    registry: &PendingRecoveryStore,
    capture: &SessionCapture,
    other: &SessionCapture,
    id: uuid::Uuid,
    prior: uuid::Uuid,
) {
    use garmin_services::firmware::recovery_receipt_id;
    use serde_json::Value;
    let journals = Path::new("mounted-update/transaction");
    let legacy = |journal: &mut Value| {
        journal.as_object_mut().unwrap().remove("transaction_id");
    };
    rewrite_preparation(capture.root(), legacy);
    rewrite_preparation(other.root(), |journal| {
        legacy(journal);
        let original = journal["writes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|w| w["original"].is_object())
            .unwrap();
        original["original"]["sha256"] = Value::from("a".repeat(64));
    });
    assert_eq!(
        recovery_receipt_id(connection, registry, None)
            .await
            .unwrap(),
        id
    );
    assert!(
        recovery_receipt_id(connection, registry, Some(prior))
            .await
            .is_err()
    );
    for name in ["000000-prepared.json", "committed.json"] {
        std::fs::copy(
            capture.root().join(journals).join(name),
            other.root().join(journals).join(name),
        )
        .unwrap();
    }
    assert!(
        recovery_receipt_id(connection, registry, None)
            .await
            .is_err()
    );
    let commit = other.root().join(journals).join("committed.json");
    std::fs::remove_file(&commit).unwrap();
    assert!(
        recovery_receipt_id(connection, registry, Some(prior))
            .await
            .unwrap_err()
            .to_string()
            .contains("histories differ")
    );
    std::fs::copy(capture.root().join(journals).join("committed.json"), commit).unwrap();
    assert_eq!(
        recovery_receipt_id(connection, registry, Some(prior))
            .await
            .unwrap(),
        prior
    );
}

fn bundle_only_receipt(manifest: &garmin_device::DeviceManifest) -> FirmwareReceipt {
    use garmin_update::firmware::{FirmwareCatalog, FirmwarePlan};
    let mut catalog = FirmwareCatalog::from_json(FENIX_GSP_22_44.catalog.as_bytes()).unwrap();
    catalog
        .updates
        .retain(|offer| offer.part_number == "006-V4532-00");
    let different_plan = FirmwarePlan::new(
        manifest.identity_digest(),
        catalog
            .updates
            .iter()
            .map(|offer| offer.download_spec(None).unwrap())
            .collect(),
    )
    .unwrap();
    FirmwareReceipt::new(manifest, different_plan.digest(), &catalog.updates).unwrap()
}

#[tokio::test]
async fn a_swapped_selection_cannot_finalize_a_consumed_bundle() {
    use garmin_services::firmware::{TransferStatus, recover_registered, status};
    use garmin_services::maps::device::Connector as _;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    let garmin = root.join("Garmin");
    std::fs::create_dir_all(&garmin).unwrap();
    std::fs::write(garmin.join("GarminDevice.xml"), OLD).unwrap();
    let capture = SessionCapture::create(&directory.path().join("capture")).unwrap();
    let registry = PendingRecoveryStore::new(directory.path().join("receipts"));
    interrupt_bundle_and_video_commit(&root, &directory.path().join("cache"), &capture, &registry)
        .await;
    let connector = DirectoryConnector {
        root: root.clone(),
        storage_label: "Internal".into(),
        storage_id: "internal".into(),
    };
    let previous = connector.connect().await.unwrap();
    let second = SessionCapture::create(&directory.path().join("second")).unwrap();
    let file = Path::new("firmware/selection.json");
    second
        .write_json_atomic(file, &bundle_only_receipt(&previous.manifest))
        .await
        .unwrap();
    let original = std::fs::read(capture.root().join(file)).unwrap();
    std::fs::copy(second.root().join(file), capture.root().join(file)).unwrap();
    let transactions = root.join("GARMIN-TOOLKIT/transactions");
    let transaction = std::fs::read_dir(transactions)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_dir(transaction.join("completed.json")).unwrap();
    let bundle = garmin.join("GUPDATE.GSP");
    let bytes = std::fs::read(&bundle).unwrap();
    std::fs::remove_file(&bundle).unwrap();
    std::fs::write(garmin.join("GarminDevice.xml"), CURRENT).unwrap();
    let device = connector.connect().await.unwrap();
    let progress = ProgressReporter::default();
    let error = recover_registered(&device, &registry, None, &progress)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("selection does not match"),
        "{error}"
    );
    assert!(matches!(
        status(&device, &registry, false).await.unwrap().as_slice(),
        [TransferStatus::Unavailable { .. }]
    ));
    assert!(status(&device, &registry, true).await.is_err());
    assert!(root.join("GARMIN-TOOLKIT/active.json").is_file());

    let mut legacy: serde_json::Value = serde_json::from_slice(&original).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("plan_digest")
        .unwrap();
    std::fs::write(
        capture.root().join(file),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    assert!(
        recover_registered(&device, &registry, None, &progress)
            .await
            .is_err()
    );
    assert!(root.join("GARMIN-TOOLKIT/active.json").is_file());
    std::fs::write(bundle, bytes).unwrap();
    recover_registered(&device, &registry, None, &progress)
        .await
        .unwrap();
    assert!(!root.join("GARMIN-TOOLKIT/active.json").exists());
    assert!(matches!(
        status(&device, &registry, true).await.unwrap().as_slice(),
        [TransferStatus::VersionsUnverified]
    ));
    assert!(
        registry
            .for_device(&device.manifest.identity_digest())
            .unwrap()
            .is_empty()
    );
}
