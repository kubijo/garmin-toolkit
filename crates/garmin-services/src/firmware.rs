//! Firmware lookup, review, and device revalidation on the connection-owning host.

mod status;
mod workflow;
pub use status::{ComponentStatus, TransferReport, TransferStatus, review_status, status};
pub use workflow::{
    approve_empty_uploads, recover_registered, recovery_receipt_id, review_empty_uploads,
};

use std::{
    future::ready,
    sync::{Arc, Mutex, PoisonError},
};

use anyhow::{Context as _, Result, ensure};
use garmin_capture::SessionCapture;
use garmin_device::DeviceManifest;
use garmin_map_service::{ClientIdentity, GARMIN_EXPRESS_USER_AGENT};
use garmin_model::artifact::ByteCount;
use garmin_model::map::MapAuthorization;
use garmin_progress::{CancellationToken, ProgressReporter, model::ProgressModel};
use garmin_service_api::firmware::{
    AvailableUpdate, Component, FirmwareService, Phase, Review, State, Transcript,
};
use garmin_simulator::shadow::Shadow;
use garmin_update::firmware::{
    FirmwareCatalog, FirmwareComponent, FirmwareOffer, FirmwarePlan, FirmwareReceipt,
    FirmwareTarget, staging_targets,
};
use garmin_update::{
    ApplyReport, AssistedRecoveryPlan, DeviceTransactionStore, DownloadError, DownloadProgress,
    DownloadSpec, Downloader,
};
use remoc::{rch, rtc};
use std::path::Path;
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

use crate::{
    deployment::Deployment,
    maps::{
        CatalogSource, MutationLocks,
        device::{Connection, Connector},
        pending_recovery::{PendingRecoveryKind, PendingRecoveryStore},
    },
};

struct Checked {
    system_upgrade: Option<String>,
    manifest: DeviceManifest,
    catalog: FirmwareCatalog,
    source: CatalogSource,
}

impl Checked {
    async fn load(
        source: &CatalogSource,
        manifest: DeviceManifest,
    ) -> Result<(Self, Vec<Component>)> {
        let identity = ClientIdentity {
            version: "8.0.0.0".into(),
            ..ClientIdentity::default()
        };
        let bytes = source
            .client_with_identity(&identity)?
            .device_software(&manifest)
            .await?;
        let catalog =
            FirmwareCatalog::from_json(&bytes).context("invalid device software catalog")?;
        let mut system_upgrade = None;
        let mut components = Vec::new();
        for item in catalog.components(manifest.capabilities())? {
            let component = item.snapshot(source.simulator_origin())?;
            if item.offers_system_upgrade() {
                system_upgrade = Some(component.available.clone());
            }
            components.push(component);
        }
        Ok((
            Self {
                system_upgrade,
                manifest,
                catalog,
                source: source.clone(),
            },
            components,
        ))
    }
}

/// Run an explicitly requested check for a locally selected target.
/// # Errors
/// Device inventory, service or catalog validation failed.
pub async fn check(source: &CatalogSource, manifest: DeviceManifest) -> Result<Vec<Component>> {
    Ok(Checked::load(source, manifest).await?.1)
}

struct Control {
    state: State,
    progress: Option<ProgressModel>,
    last_progress_publish: Option<std::time::Instant>,
    stage_cancellation: Option<CancellationToken>,
    system_upgrade: Option<String>,
    checked: Option<Checked>,
    recovery_identity: Option<String>,
    inspected_receipt: Option<Uuid>,
    recovery_review: Option<AssistedRecoveryPlan>,
}

struct Inner {
    attachment: Uuid,
    disconnected: CancellationToken,
    device: String,
    connector: Arc<dyn Connector>,
    source: CatalogSource,
    mutations: Arc<MutationLocks>,
    deployment: Option<(Arc<Deployment>, Uuid)>,
    control: Mutex<Control>,
    snapshots: rch::watch::Sender<State>,
}

#[derive(Clone)]
pub struct Session(Arc<Inner>);

impl Session {
    pub(crate) fn new(
        device: String,
        connector: Arc<dyn Connector>,
        source: CatalogSource,
        mutations: Arc<MutationLocks>,
        deployment: Option<(Arc<Deployment>, Uuid)>,
    ) -> Self {
        let state = State {
            transcript: Transcript::default(),
            simulated: matches!(source, CatalogSource::Loopback(_)),
            has_retained_transfer: false,
            revision: Uuid::new_v4(),
            phase: Phase::Consent,
            review: None,
            inspection: None,
            catalog_id: Uuid::nil(),
        };
        let (snapshots, _) = rch::watch::channel(state.clone());
        Self(Arc::new(Inner {
            attachment: state.revision,
            disconnected: CancellationToken::default(),
            device,
            connector,
            source,
            mutations,
            deployment,
            control: Mutex::new(Control {
                state,
                progress: None,
                last_progress_publish: None,
                stage_cancellation: None,
                system_upgrade: None,
                checked: None,
                recovery_identity: None,
                inspected_receipt: None,
                recovery_review: None,
            }),
            snapshots,
        }))
    }

    #[must_use]
    pub fn snapshot(&self) -> State {
        self.0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .state
            .clone()
    }

    fn publish(&self, control: &mut Control) -> State {
        control.state.revision = Uuid::new_v4();
        self.0.snapshots.send_replace(control.state.clone());
        control.state.clone()
    }

    /// Only an untouched session can claim the initial revision. A manual check can claim it first.
    pub fn check_on_attachment(&self) {
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if control.state.revision == self.0.attachment {
            self.begin_check(&mut control);
        }
    }

    pub(crate) fn disconnect(&self) {
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.0.disconnected.cancel();
        if let Some(token) = control.stage_cancellation.take() {
            token.cancel();
        }
        control.recovery_review = None;
        control.checked = None;
        control.system_upgrade = None;
        control.state.review = None;
        control.state.phase = Phase::Disconnected;
        control.progress = None;
        control.state.inspection = None;
        self.publish(&mut control);
    }

    fn recovery_progress(&self) -> Result<ProgressReporter> {
        ensure!(
            !self.0.disconnected.is_cancelled(),
            "device was disconnected"
        );
        Ok(ProgressReporter::default().with_cancellation(self.0.disconnected.clone()))
    }

    #[must_use]
    pub fn available_update(&self) -> Option<AvailableUpdate> {
        let control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Some(AvailableUpdate {
            attachment: self.0.attachment,
            version: control.system_upgrade.clone()?,
        })
    }

    /// Wait for an explicitly requested check and return the catalog revision with its components.
    /// # Errors
    /// The host could not check the selected device.
    pub async fn check_components(&self) -> Result<(Uuid, Vec<Component>)> {
        let mut watch = self.0.snapshots.subscribe();
        self.start_check(self.snapshot().revision)?;
        loop {
            let state = watch.borrow_and_update()?.clone();
            match state.phase {
                Phase::Available(components) => return Ok((state.revision, components)),
                Phase::Failed(message) => anyhow::bail!("{message}"),
                Phase::Checking => watch.changed().await?,
                _ => anyhow::bail!("firmware check was superseded"),
            }
        }
    }

    /// Start one lookup. Opening or observing a session never contacts the service.
    /// # Errors
    /// Another check is running or the supplied revision has expired.
    pub fn start_check(&self, revision: Uuid) -> Result<State> {
        let mut c = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ensure!(c.state.revision == revision, "firmware catalog changed");
        ensure!(
            c.state.inspection.is_none(),
            "close the transfer receipt first"
        );
        ensure!(
            !matches!(
                c.state.phase,
                Phase::Checking | Phase::Staging(_) | Phase::Recovering | Phase::Disconnected
            ),
            "firmware operation already running"
        );
        Ok(self.begin_check(&mut c))
    }

    fn begin_check(&self, c: &mut Control) -> State {
        c.state.transcript = Transcript::default();
        c.checked = None;
        c.state.review = None;
        c.state.phase = Phase::Checking;
        let state = self.publish(c);
        let session = self.clone();
        tokio::spawn(async move {
            let result = session.load().await;
            let mut c = session
                .0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if c.state.phase == Phase::Disconnected {
                return;
            }
            match result {
                Ok((mut catalog, components)) => {
                    c.state.catalog_id = Uuid::new_v4();
                    c.system_upgrade = catalog.system_upgrade.take();
                    c.checked = Some(catalog);
                    c.state.phase = Phase::Available(components);
                }
                Err(error) => c.state.phase = Phase::Failed(error.to_string()),
            }
            session.publish(&mut c);
        });
        state
    }

    async fn load(&self) -> Result<(Checked, Vec<Component>)> {
        let _lease = self.lease().await?;
        let manifest = self.0.connector.connect().await?.manifest;
        if let Some((deployment, _)) = &self.0.deployment {
            let receipts = self.receipt_store(&deployment.root().join("firmware"))?;
            let retained = !receipts.for_device(&manifest.identity_digest())?.is_empty();
            self.0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .state
                .has_retained_transfer = retained;
        }
        Checked::load(&self.0.source, manifest).await
    }

    fn receipt_store(&self, root: &Path) -> Result<PendingRecoveryStore> {
        if self.0.source.simulator_origin().is_some() {
            Ok(PendingRecoveryStore::new(root.join("receipts")))
        } else {
            PendingRecoveryStore::physical_firmware()
        }
    }

    async fn lease(&self) -> Result<Option<crate::deployment::ApplicationLease>> {
        match &self.0.deployment {
            Some((host, epoch)) => Ok(Some(host.application_owned(*epoch).await?)),
            None => Ok(None),
        }
    }

    /// Prepare a single-use approval bound to this catalog and selection.
    /// # Errors
    /// The catalog revision or selected components are unavailable.
    pub fn review_selection(&self, revision: Uuid, mut selected: Vec<u32>) -> Result<State> {
        let mut c = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ensure!(c.state.revision == revision, "firmware catalog changed");
        let Phase::Available(components) = &c.state.phase else {
            anyhow::bail!("check for device updates first");
        };
        selected.sort_unstable();
        ensure!(
            !selected.is_empty() && selected.windows(2).all(|pair| pair[0] != pair[1]),
            "select distinct firmware components"
        );
        let mut total = ByteCount::default();
        for &index in &selected {
            let item = components
                .get(index as usize)
                .context("unknown firmware component")?;
            if let Some(reason) = &item.staging_issue {
                anyhow::bail!("{reason}");
            }
            let bytes = item
                .download_bytes
                .context("component is unavailable for staging")?;
            total = total
                .checked_add(bytes)
                .context("firmware selection size overflow")?;
        }
        c.state.review = Some(Review {
            approval: Uuid::new_v4(),
            selected,
            download_bytes: total,
        });
        Ok(self.publish(&mut c))
    }

    /// Consume the reviewed choice under the shared maps/device write lock and reread the target.
    /// # Errors
    /// Rejects a stale or consumed approval, a busy device, or changed inventory.
    pub async fn claim_for_staging(
        &self,
        revision: Uuid,
        approval: Uuid,
    ) -> Result<ApprovedSelection> {
        self.claim(revision, approval, Phase::Consent).await
    }

    async fn claim(
        &self,
        revision: Uuid,
        approval: Uuid,
        phase: Phase,
    ) -> Result<ApprovedSelection> {
        let lease = self.lease().await?;
        let guard = self
            .0
            .mutations
            .acquire(&self.0.device)
            .map_err(anyhow::Error::msg)?;
        let (checked, review) = {
            let mut c = self
                .0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            ensure!(
                c.state.inspection.is_none()
                    && c.state.revision == revision
                    && c.state
                        .review
                        .as_ref()
                        .is_some_and(|r| r.approval == approval),
                "firmware approval expired"
            );
            let review = c.state.review.take().context("no firmware review")?;
            let checked = c.checked.take().context("no firmware catalog")?;
            c.state.phase = phase;
            self.publish(&mut c);
            (checked, review)
        };
        let connection = self.0.connector.connect().await?;
        let fresh = &connection.manifest;
        check_identity(&checked.manifest, fresh)?;
        let before = checked
            .catalog
            .components(checked.manifest.capabilities())?;
        let mut keys = std::collections::HashSet::new();
        for i in review.selected {
            let item = &before[i as usize];
            for offer in std::iter::once(item.offer).chain(item.metadata) {
                check_installed(offer, &checked.manifest, fresh)?;
                keys.insert((offer.part_number.clone(), offer.semantic_version.clone()));
            }
        }
        let mut offers: Vec<_> = checked
            .catalog
            .updates
            .into_iter()
            .filter(|offer| {
                keys.contains(&(offer.part_number.clone(), offer.semantic_version.clone()))
            })
            .collect();
        offers.sort_by_key(|offer| offer.installation.order);
        let targets =
            staging_targets(&offers, fresh.capabilities(), connection.device.as_ref()).await?;
        Ok(ApprovedSelection {
            connection,
            session: self.clone(),
            virtual_target: false,
            connector: Arc::clone(&self.0.connector),
            offers,
            targets,
            source: checked.source,
            _guard: guard,
            _lease: lease,
        })
    }
}

fn check_identity(previous: &DeviceManifest, fresh: &DeviceManifest) -> Result<()> {
    ensure!(
        previous.identity_digest() == fresh.identity_digest()
            && previous.summary.part_number == fresh.summary.part_number,
        "selected device changed"
    );
    Ok(())
}

fn check_installed(
    offer: &FirmwareOffer,
    previous: &DeviceManifest,
    fresh: &DeviceManifest,
) -> Result<()> {
    let previous = previous.capabilities();
    let fresh = fresh.capabilities();
    ensure!(
        offer.installed_file(previous)? == offer.installed_file(fresh)?
            && previous.update_storages(&offer.part_number)
                == fresh.update_storages(&offer.part_number),
        "installed firmware inventory changed"
    );
    Ok(())
}

pub struct ApprovedSelection {
    connection: Connection,
    session: Session,
    virtual_target: bool,
    connector: Arc<dyn Connector>,
    offers: Vec<FirmwareOffer>,
    targets: Vec<FirmwareTarget>,
    source: CatalogSource,
    _guard: OwnedMutexGuard<()>,
    _lease: Option<crate::deployment::ApplicationLease>,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "The new transfer has not started. Inspect the previous transfer with `firmware status` before writing another update to this device."
)]
struct PreviousTransferPending;

impl ApprovedSelection {
    /// Run the transfer against an isolated, retained virtual MTP device.
    #[must_use]
    pub const fn on_virtual_device(mut self) -> Self {
        self.virtual_target = true;
        self
    }

    /// Stage the approved files and keep verified originals and recovery evidence in the host capture.
    /// # Errors
    /// Rejects changed inventory, incorrect update metadata, and failed or interrupted writes.
    pub async fn stage(
        self,
        cache: &Path,
        capture: &SessionCapture,
        receipts: &PendingRecoveryStore,
        progress: ProgressReporter,
    ) -> Result<ApplyReport> {
        let progress = progress.with_cleanup_cancellation(self.session.0.disconnected.clone());
        {
            let mut control = self
                .session
                .0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            ensure!(
                control.state.phase != Phase::Disconnected,
                "device was disconnected; review the new attachment"
            );
            control.stage_cancellation = Some(progress.cancellation_token());
        }
        let previous = &self.connection.manifest;
        let device_digest = previous.identity_digest();
        let _slot = receipts.lock_device(&device_digest)?;
        ensure!(
            receipts.for_device(&device_digest)?.is_empty(),
            PreviousTransferPending
        );
        let verified = self.download(cache, progress.clone()).await?;
        ensure!(!progress.is_cancelled(), "firmware transfer cancelled");
        let fresh = self.connector.connect().await?;
        check_identity(previous, &fresh.manifest)?;
        for offer in &self.offers {
            check_installed(offer, previous, &fresh.manifest)?;
        }
        let targets = staging_targets(
            &self.offers,
            fresh.manifest.capabilities(),
            fresh.device.as_ref(),
        )
        .await?;
        ensure!(
            targets == self.targets,
            "reviewed firmware destination changed"
        );
        let plan = FirmwarePlan::new(device_digest.clone(), self.downloads()?)?;
        let receipt = FirmwareReceipt::new(&fresh.manifest, plan.digest(), &self.offers)?;
        capture
            .write_json_atomic(Path::new("firmware/selection.json"), &receipt)
            .await?;
        let shadow = if self.virtual_target {
            Some(
                Shadow::create(
                    fresh.device.as_ref(),
                    &fresh.manifest,
                    plan.update_plan(),
                    &MapAuthorization::default(),
                    capture,
                    &progress,
                )
                .await?,
            )
        } else {
            None
        };
        let device = shadow
            .as_ref()
            .map_or(fresh.device.as_ref(), Shadow::device);
        ensure!(!progress.is_cancelled(), "firmware transfer cancelled");
        let pending = receipts.register(
            PendingRecoveryKind::Update,
            capture.root(),
            &device_digest,
            plan.digest(),
        )?;
        let outcome = plan
            .stage(&verified, &targets, device, capture, progress)
            .await;
        if outcome.is_err()
            && !pending.is_prepared()?
            && status::read_selection(&pending.transaction)
                .await
                .is_ok_and(|r| {
                    r.is_bound_to(&fresh.manifest, &pending.plan_digest)
                        .is_ok_and(|bound| bound)
                })
            && DeviceTransactionStore::open(device)
                .await?
                .active_plan_digest(&device_digest)
                .await?
                .is_none()
        {
            receipts.discard_unprepared(&pending)?;
        }
        if let Some(shadow) = shadow {
            shadow.finish(capture, outcome.is_ok()).await?;
        }
        Ok(outcome?)
    }

    fn downloads(&self) -> Result<Vec<DownloadSpec>> {
        self.offers
            .iter()
            .map(|offer| {
                offer
                    .download_spec(self.source.simulator_origin())
                    .context("invalid firmware deliverable")
            })
            .collect()
    }

    /// Cache and verify the complete selection, then validate companion transitions before any device write.
    /// # Errors
    /// A delivery failed validation or a companion describes a different installed or offered version.
    pub async fn download(
        &self,
        cache: &Path,
        progress: ProgressReporter,
    ) -> Result<Vec<DownloadProgress>> {
        let specs = self.downloads()?;
        let downloader = match self.source {
            CatalogSource::Garmin => Downloader::new(2, GARMIN_EXPRESS_USER_AGENT)?,
            CatalogSource::Loopback(_) => Downloader::local_mock(2, GARMIN_EXPRESS_USER_AGENT)?,
        };
        let files = downloader
            .stage_all_with_progress(&specs, cache, progress)
            .await
            .map_err(|error| match error {
                DownloadError::Request(error) => DownloadError::Request(error.without_url()),
                error => error,
            })?;
        for ((offer, file), spec) in self
            .offers
            .iter()
            .zip(&files)
            .zip(&specs)
            .filter(|((offer, _), _)| offer.data_type == "UpdateMetadata")
        {
            let payload = self
                .offers
                .iter()
                .find(|parent| {
                    Some(parent.part_number.as_str()) == offer.part_number.strip_suffix("-META")
                        && parent.semantic_version == offer.semantic_version
                })
                .context("missing firmware metadata parent")?;
            let component = FirmwareComponent {
                offer: payload,
                metadata: Some(offer),
                installed: payload.installed_file(self.connection.manifest.capabilities())?,
            };
            let record =
                garmin_update::firmware::UpdateMetadata::read_verified(&file.path, spec).await?;
            ensure!(
                component.matches_transition(&record),
                "firmware metadata describes a different version transition"
            );
        }
        Ok(files)
    }

    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.connection
    }

    #[must_use]
    pub fn offers(&self) -> &[FirmwareOffer] {
        &self.offers
    }

    #[must_use]
    pub fn targets(&self) -> &[FirmwareTarget] {
        &self.targets
    }
}

impl FirmwareService for Session {
    fn act(
        &self,
        revision: Uuid,
        action: garmin_service_api::firmware::Action,
    ) -> impl Future<Output = Result<Result<State, String>, rtc::CallError>> {
        ready(Ok(self.submit(revision, action).map_err(|e| e.to_string())))
    }
    fn watch(&self) -> impl Future<Output = Result<rch::watch::Receiver<State>, rtc::CallError>> {
        ready(Ok(self.0.snapshots.subscribe()))
    }
    fn check(
        &self,
        revision: Uuid,
    ) -> impl Future<Output = Result<Result<State, String>, rtc::CallError>> {
        ready(Ok(self.start_check(revision).map_err(|e| e.to_string())))
    }
    fn review(
        &self,
        revision: Uuid,
        selected: Vec<u32>,
    ) -> impl Future<Output = Result<Result<State, String>, rtc::CallError>> {
        ready(Ok(self
            .review_selection(revision, selected)
            .map_err(|e| e.to_string())))
    }
}
