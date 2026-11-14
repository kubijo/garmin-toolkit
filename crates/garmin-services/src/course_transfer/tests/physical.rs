//! Host-outcome interruption on a real or disposable device;
//! no cable-pull failure is claimed.

use super::*;
use crate::maps::device::MountedConnector;

mod workout;

struct BlockedOutcome(PathBuf);

impl BlockedOutcome {
    fn new(directory: &Path) -> std::io::Result<Self> {
        let path = directory.join("outcome.json");
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for BlockedOutcome {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn recovers_after_outcome_failure_on_a_disposable_device() -> Result<()> {
    let root = tempfile::tempdir()?;
    let watch = Device::open(root.path().join("watch"))?;
    let connector: Arc<dyn Connector> = Arc::new(DirectoryConnector {
        root: watch.root().to_owned(),
        storage_id: fixture::STORAGE_ID.to_owned(),
        storage_label: fixture::STORAGE_LABEL.to_owned(),
    });
    recover_course(&root.path().join("host"), fixture::KEY, connector).await
}

/// Leaves TK Recovery on the device for owner confirmation after disconnect.
#[tokio::test(flavor = "current_thread")]
#[ignore = "physical Course write: requires owner consent, model selection, and an unused evidence directory"]
async fn physical_course_outcome_recovery() -> Result<()> {
    let model = std::env::var("GARMIN_TOOLKIT_ACCEPTANCE_MODEL")?;
    let root = PathBuf::from(std::env::var("GARMIN_TOOLKIT_COURSE_ACCEPTANCE_DIR")?);
    fs::create_dir(&root)?;
    let candidates = garmin_device::discover_mounted_mtp()
        .into_iter()
        .filter(|candidate| candidate.name == model)
        .collect::<Vec<_>>();
    let [candidate] = candidates.as_slice() else {
        bail!("expected exactly one selected mounted device");
    };
    let connector: Arc<dyn Connector> = Arc::new(MountedConnector(candidate.mount_id.clone()));
    let connection = connector.connect().await?;
    ensure!(
        connection.manifest.summary.model == model,
        "selected model changed"
    );
    drop(connection);
    recover_course(&root, &candidate.mount_id, connector).await
}

async fn recover_course(
    root: &Path,
    device_key: &str,
    connector: Arc<dyn Connector>,
) -> Result<()> {
    let deployment = Deployment::open(root).await?;
    let (owner, _, operation, _) = generated_course_named(&deployment, "TK Recovery")
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    let transfers =
        CourseTransfers::new(Arc::clone(&deployment), Arc::new(MutationLocks::default()));
    let targets = transfers
        .targets(owner, operation, device_key, connector.as_ref())
        .await?;
    let [target] = targets.as_slice() else {
        bail!("requires exactly one manifest-declared Course destination");
    };
    let connection = connector.connect().await?;
    let pairing =
        garmin_update::read_profile_marker(connection.device.as_ref(), &target.storage_id)
            .await?
            .map(|marker| marker.bytes())
            .transpose()?;
    let CourseTransferPreparation::Review(review) = transfers
        .prepare(
            owner,
            operation,
            device_key.to_owned(),
            &target.storage_id,
            Arc::clone(&connector),
        )
        .await?
    else {
        bail!("expected a new synthetic Course review");
    };
    let directory = interrupt_outcome(&transfers, owner, &review).await?;
    let intent = read_intent(&directory)?;
    let payload = fs::read(directory.join("payload.fit"))?;
    let observed = connection
        .device
        .read_bounded_file(&target.storage_id, &intent.path, review.byte_count.as_u64())
        .await?
        .context("uploaded Course is missing")?;
    ensure!(
        observed == payload,
        "device Course differs from retained payload"
    );
    drop(transfers);

    let recovered =
        CourseTransfers::new(Arc::clone(&deployment), Arc::new(MutationLocks::default()));
    ensure!(
        matches!(
            recovered
                .status(owner, review.transfer, connector.as_ref())
                .await?
                .phase,
            CourseTransferPhase::Verified
        ),
        "transfer-service restart did not recover the exact Course"
    );
    ensure!(
        matches!(
            recovered.prepare(owner, operation, device_key.to_owned(), &target.storage_id, Arc::clone(&connector)).await?,
            CourseTransferPreparation::AlreadyOnDevice(copy) if copy.transfer == review.transfer
        ),
        "recovery offered a duplicate send"
    );
    let after = garmin_update::read_profile_marker(connection.device.as_ref(), &target.storage_id)
        .await?
        .map(|marker| marker.bytes())
        .transpose()?;
    ensure!(
        after == pairing,
        "device pairing changed during Course transfer"
    );
    println!(
        "TK Recovery: exact device bytes recovered after failed outcome persistence; duplicate send suppressed; pairing unchanged."
    );
    deployment.close().await;
    Ok(())
}

async fn interrupt_outcome(
    transfers: &Arc<CourseTransfers>,
    owner: UserContext,
    review: &CourseTransferReview,
) -> Result<PathBuf> {
    let started = transfers.approve(owner, review.approval).await?;
    ensure!(
        matches!(started.phase, CourseTransferPhase::Running),
        "upload did not start"
    );
    let directory = transfers.root.join(review.transfer.to_string());
    // Current-thread Tokio cannot poll the spawned writer before this synchronous fault is armed.
    let blocked = BlockedOutcome::new(&directory)?;
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            if !transfers
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&review.transfer)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    ensure!(blocked.0.is_dir(), "outcome fault was not retained");
    drop(blocked);
    ensure!(
        read_outcome(&directory)?.is_none(),
        "unexpected completed outcome"
    );
    Ok(directory)
}
