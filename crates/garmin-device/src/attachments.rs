//! Platform-neutral attached-device lifecycle.

use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::Duration,
};

use crate::{DataType, DeviceId, SoftwareVersion, TransferDirection};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

/// A recognizable attachment.
pub trait Candidate: Clone + Send + 'static {
    /// Opaque identity for the current attachment.
    fn key(&self) -> &str;

    /// Host-provided display name.
    fn name(&self) -> &str;

    /// Reads device metadata.
    /// # Errors
    /// A stable inspection failure.
    fn inspect(&self) -> Result<Metadata, String>;

    /// Takes a bounded, read-only snapshot for an explicitly opened browser.
    /// # Errors
    /// The adapter does not support browsing or the attachment changed.
    fn browse(&self) -> Result<crate::DeviceCatalog, String> {
        Err("device browsing is unsupported by this adapter".to_owned())
    }

    /// Takes a bounded snapshot while honoring operation cancellation.
    /// # Errors
    /// The adapter does not support browsing, the attachment changed, or cancellation was
    /// requested.
    fn browse_with_progress(
        &self,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<crate::DeviceCatalog, String> {
        if progress.is_cancelled() {
            return Err("device browsing was cancelled".to_owned());
        }
        let catalog = self.browse()?;
        if progress.is_cancelled() {
            return Err("device browsing was cancelled".to_owned());
        }
        Ok(catalog)
    }
}

/// Attachment discovery.
pub trait Backend {
    /// Candidate produced by this adapter.
    type Candidate: Candidate;

    /// Dispatches platform events and reports stale discovery state.
    fn poll_changed(&self) -> bool;

    /// The current attachment snapshot without inspecting contents.
    fn candidates(&self) -> Vec<Self::Candidate>;
}

/// An attachment change.
pub enum Event {
    Attached {
        /// Opaque attachment identity.
        key: String,
        /// Host-provided name.
        name: String,
    },
    Detached {
        /// Opaque attachment identity.
        key: String,
    },
    Inspected {
        /// Opaque attachment identity.
        key: String,
        /// Manifest-provided device name.
        name: String,
    },
    InspectionFailed {
        /// Opaque attachment identity.
        key: String,
        /// Best available device name.
        name: String,
        /// Stable failure summary.
        reason: String,
    },
}

/// Display data for an attachment.
pub struct Presentation {
    pub key: String,
    pub name: String,
    pub identifier: Option<DeviceId>,
    pub software_version: Option<SoftwareVersion>,
    pub capabilities: Vec<Capability>,
    pub storage: Option<crate::DeviceStateSnapshot>,
    pub state: InspectionState,
    pub inspection_error: Option<String>,
    pub report: Option<garmin_model::device::DeviceInspection>,
}

/// A supported transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    data_type: DataType,
    direction: TransferDirection,
}

impl Capability {
    #[must_use]
    pub const fn new(data_type: DataType, direction: TransferDirection) -> Self {
        Self {
            data_type,
            direction,
        }
    }

    #[must_use]
    pub const fn data_type(self) -> DataType {
        self.data_type
    }

    #[must_use]
    pub const fn direction(self) -> TransferDirection {
        self.direction
    }
}

/// Inspection state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionState {
    Running,
    Ready,
    Failed,
}

type Inspector<C> = dyn Fn(&C) -> Result<Metadata, String> + Send + Sync;

/// Attached-device coordinator.
pub struct Manager<B>
where
    B: Backend,
{
    backend: B,
    attachments: Vec<Attachment<B::Candidate>>,
    last_reconcile: Option<std::time::Instant>,
    inspection_tx: mpsc::Sender<InspectionResult>,
    inspection_rx: mpsc::Receiver<InspectionResult>,
    next_generation: u64,
    inspector: Arc<Inspector<B::Candidate>>,
}

impl<B> Manager<B>
where
    B: Backend,
{
    #[must_use]
    pub fn new(backend: B) -> Self {
        Self::with_inspector(backend, Candidate::inspect)
    }

    #[must_use]
    pub fn with_inspector(
        backend: B,
        inspector: impl Fn(&B::Candidate) -> Result<Metadata, String> + Send + Sync + 'static,
    ) -> Self {
        let (inspection_tx, inspection_rx) = mpsc::channel();
        Self {
            backend,
            attachments: Vec::new(),
            last_reconcile: None,
            inspection_tx,
            inspection_rx,
            next_generation: 0,
            inspector: Arc::new(inspector),
        }
    }

    /// Reconciles attachments and collects inspections.
    pub fn poll(&mut self) -> Vec<Event> {
        let now = std::time::Instant::now();
        let initial = self.last_reconcile.is_none();
        let changed = self.backend.poll_changed();
        let due = self
            .last_reconcile
            .is_none_or(|last| now.duration_since(last) >= RECONCILE_INTERVAL);
        let mut events = if changed || due {
            self.last_reconcile = Some(now);
            self.reconcile(!initial, changed)
        } else {
            Vec::new()
        };
        events.extend(self.collect_inspections());
        events.extend(self.start_inspections());
        events
    }

    #[must_use]
    pub fn presentations(&self) -> Vec<Presentation> {
        self.attachments
            .iter()
            .map(|attachment| {
                let metadata = match &attachment.inspection {
                    Inspection::Ready(metadata) => Some(metadata),
                    _ => attachment.previous.as_ref(),
                };
                let state = match &attachment.inspection {
                    Inspection::Available | Inspection::Running => InspectionState::Running,
                    Inspection::Ready(metadata)
                        if metadata
                            .report
                            .as_ref()
                            .is_some_and(garmin_model::device::DeviceInspection::has_errors) =>
                    {
                        InspectionState::Failed
                    }
                    Inspection::Ready(_) => InspectionState::Ready,
                    Inspection::Failed(_) => InspectionState::Failed,
                };
                Presentation {
                    key: attachment.key.clone(),
                    name: metadata
                        .map_or_else(|| attachment.name.clone(), |metadata| metadata.name.clone()),
                    identifier: metadata.and_then(|metadata| metadata.id),
                    software_version: metadata.and_then(|metadata| metadata.software_version),
                    capabilities: metadata
                        .map_or_else(Vec::new, |metadata| metadata.capabilities.clone()),
                    storage: metadata.map(|metadata| metadata.storage.clone()),
                    report: metadata.and_then(|metadata| metadata.report.clone()),
                    state,
                    inspection_error: match &attachment.inspection {
                        Inspection::Failed(reason) => Some(reason.clone()),
                        _ => None,
                    },
                }
            })
            .collect()
    }

    /// Requests one refresh, retaining the last result while it runs.
    /// # Errors
    /// The attachment is no longer present.
    pub fn refresh(&mut self, key: &str) -> Result<(), String> {
        let attachment = self
            .attachments
            .iter_mut()
            .find(|attachment| attachment.key == key)
            .ok_or_else(|| "the selected device is no longer connected".to_owned())?;
        if matches!(
            attachment.inspection,
            Inspection::Available | Inspection::Running
        ) {
            return Ok(());
        }
        attachment.retain_previous();
        attachment.inspection = Inspection::Available;
        attachment.generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        Ok(())
    }

    /// Requires a read after a possibly mutating operation, even if a read is in flight.
    pub fn invalidate(&mut self, key: &str) {
        if let Some(attachment) = self.attachments.iter_mut().find(|item| item.key == key)
            && matches!(attachment.inspection, Inspection::Running)
        {
            attachment.refresh_pending = true;
        } else {
            let _refresh = self.refresh(key);
        }
    }

    #[must_use]
    pub fn candidate(&self, key: &str) -> Option<B::Candidate> {
        self.attachments
            .iter()
            .find(|attachment| attachment.key == key)
            .map(|attachment| attachment.candidate.clone())
    }

    #[must_use]
    pub fn inspecting_name(&self) -> Option<&str> {
        self.attachments
            .iter()
            .find(|attachment| matches!(attachment.inspection, Inspection::Running))
            .map(|attachment| attachment.name.as_str())
    }

    fn reconcile(&mut self, report_arrivals: bool, refresh_existing: bool) -> Vec<Event> {
        let mut previous = std::mem::take(&mut self.attachments)
            .into_iter()
            .map(|attachment| (attachment.key.clone(), attachment))
            .collect::<HashMap<_, _>>();
        let mut events = Vec::new();
        for candidate in self.backend.candidates() {
            let key = candidate.key().to_owned();
            if let Some(mut attachment) = previous.remove(&key) {
                attachment.candidate = candidate;
                if refresh_existing {
                    attachment.retain_previous();
                    attachment.inspection = Inspection::Available;
                    attachment.refresh_pending = false;
                    attachment.generation = self.next_generation;
                    self.next_generation = self.next_generation.wrapping_add(1);
                }
                self.attachments.push(attachment);
            } else {
                let name = candidate.name().to_owned();
                if report_arrivals {
                    events.push(Event::Attached {
                        key: key.clone(),
                        name: name.clone(),
                    });
                }
                self.attachments.push(Attachment {
                    key,
                    name,
                    candidate,
                    inspection: Inspection::Available,
                    previous: None,
                    refresh_pending: false,
                    generation: self.next_generation,
                });
                self.next_generation = self.next_generation.wrapping_add(1);
            }
        }
        events.extend(previous.into_keys().map(|key| Event::Detached { key }));
        events
    }

    fn start_inspections(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for attachment in &mut self.attachments {
            if !matches!(attachment.inspection, Inspection::Available) {
                continue;
            }
            attachment.inspection = Inspection::Running;
            let candidate = attachment.candidate.clone();
            let key = attachment.key.clone();
            let generation = attachment.generation;
            let sender = self.inspection_tx.clone();
            let inspector = Arc::clone(&self.inspector);
            if let Err(error) = std::thread::Builder::new()
                .name("garmin-toolkit-device-inspection".to_owned())
                .spawn(move || {
                    let result = inspector(&candidate);
                    let _ignored = sender.send(InspectionResult {
                        key,
                        generation,
                        result,
                    });
                })
            {
                let reason = format!("could not start device inspection: {error}");
                attachment.inspection = Inspection::Failed(reason.clone());
                events.push(Event::InspectionFailed {
                    key: attachment.key.clone(),
                    name: attachment.name.clone(),
                    reason,
                });
            }
        }
        events
    }

    fn collect_inspections(&mut self) -> Vec<Event> {
        self.inspection_rx
            .try_iter()
            .filter_map(
                |InspectionResult {
                     key,
                     generation,
                     result,
                 }| {
                    let attachment = self.attachments.iter_mut().find(|attachment| {
                        attachment.key == key && attachment.generation == generation
                    })?;
                    if attachment.refresh_pending {
                        attachment.refresh_pending = false;
                        attachment.inspection = Inspection::Available;
                        return None;
                    }
                    Some(match result {
                        Ok(metadata) => {
                            let name = metadata.name.clone();
                            attachment.inspection = Inspection::Ready(metadata);
                            Event::Inspected { key, name }
                        }
                        Err(reason) => {
                            attachment.inspection = Inspection::Failed(reason.clone());
                            Event::InspectionFailed {
                                key,
                                name: attachment.name.clone(),
                                reason,
                            }
                        }
                    })
                },
            )
            .collect()
    }
}

struct Attachment<C> {
    key: String,
    name: String,
    candidate: C,
    inspection: Inspection,
    previous: Option<Metadata>,
    refresh_pending: bool,
    generation: u64,
}

impl<C> Attachment<C> {
    fn retain_previous(&mut self) {
        if let Inspection::Ready(metadata) = &self.inspection {
            self.previous = Some(metadata.clone());
        }
    }
}

enum Inspection {
    Available,
    Running,
    Ready(Metadata),
    Failed(String),
}

/// Validated metadata produced by a target adapter.
#[derive(Clone)]
pub struct Metadata {
    pub id: Option<DeviceId>,
    pub device_digest: Option<String>,
    pub report: Option<garmin_model::device::DeviceInspection>,
    pub name: String,
    pub software_version: Option<SoftwareVersion>,
    pub capabilities: Vec<Capability>,
    pub storage: crate::DeviceStateSnapshot,
}

struct InspectionResult {
    key: String,
    generation: u64,
    result: Result<Metadata, String>,
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };

    use super::*;

    #[derive(Clone)]
    struct TestCandidate {
        key: &'static str,
        inspections: Arc<AtomicUsize>,
        inspected: mpsc::Sender<()>,
    }

    impl Candidate for TestCandidate {
        fn key(&self) -> &str {
            self.key
        }

        fn name(&self) -> &'static str {
            "Mock Inspect-o-Matic 9000"
        }

        fn inspect(&self) -> Result<Metadata, String> {
            self.inspections.fetch_add(1, Ordering::Relaxed);
            self.inspected.send(()).map_err(|error| error.to_string())?;
            Ok(metadata(42))
        }
    }

    struct TestBackend {
        candidates: Arc<Mutex<Vec<TestCandidate>>>,
        changed: Arc<AtomicBool>,
    }

    impl Backend for TestBackend {
        type Candidate = TestCandidate;

        fn poll_changed(&self) -> bool {
            self.changed.swap(false, Ordering::Relaxed)
        }

        fn candidates(&self) -> Vec<Self::Candidate> {
            self.candidates
                .lock()
                .expect("the test retains no poisoned lock")
                .clone()
        }
    }

    #[test]
    fn startup_inventory_automatically_inspects_recognized_devices() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);

        let events = manager.poll();

        assert!(events.is_empty());
        fixture
            .inspected
            .recv_timeout(Duration::from_secs(1))
            .expect("automatic inspection starts");
        assert_eq!(fixture.inspections.load(Ordering::Relaxed), 1);
        let presentations = manager.presentations();
        assert_eq!(presentations.len(), 1);
        assert_eq!(presentations[0].key, "test://mock-cycle");
        assert_eq!(presentations[0].state, InspectionState::Running);
    }

    #[test]
    fn attachment_after_startup_emits_an_arrival() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _startup = manager.poll();
        fixture
            .inspected
            .recv_timeout(Duration::from_secs(1))
            .expect("startup inspection completes");
        let _inspected = manager.poll();
        let mut mock_watch = fixture
            .candidates
            .lock()
            .expect("the test retains no poisoned lock")[0]
            .clone();
        mock_watch.key = "test://mock-watch";
        fixture
            .candidates
            .lock()
            .expect("the test retains no poisoned lock")
            .push(mock_watch);
        fixture.changed.store(true, Ordering::Relaxed);

        let events = manager.poll();

        assert!(events.iter().any(
            |event| matches!(event, Event::Attached { key, .. } if key == "test://mock-watch")
        ));
        fixture
            .inspected
            .recv_timeout(Duration::from_secs(1))
            .expect("new attachment is inspected automatically");
    }

    #[test]
    fn automatic_inspection_enriches_the_same_attachment() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _events = manager.poll();

        fixture
            .inspected
            .recv_timeout(Duration::from_secs(1))
            .expect("the inspection worker completes");
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let events = loop {
            let events = manager.poll();
            if !events.is_empty() {
                break events;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the coordinator receives the completed inspection"
            );
            std::thread::yield_now();
        };

        assert!(matches!(events.as_slice(), [Event::Inspected { key, name }]
                if key == "test://mock-cycle" && name == "Mock Cycle-o-Matic 9000"));
        let presentations = manager.presentations();
        assert_eq!(presentations[0].identifier, Some(DeviceId::from_u32(42)));
        assert_eq!(
            presentations[0].software_version,
            Some(SoftwareVersion::from_hundredths(912))
        );
        assert_eq!(presentations[0].state, InspectionState::Ready);
        assert_eq!(
            presentations[0].capabilities,
            [Capability::new(
                DataType::Activity,
                TransferDirection::OutputFromUnit,
            )]
        );
    }

    #[test]
    fn mount_change_refreshes_completed_inspection() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _startup = manager.poll();
        fixture
            .inspected
            .recv_timeout(Duration::from_secs(1))
            .expect("startup inspection completes");
        wait_until_ready(&mut manager);

        fixture.changed.store(true, Ordering::Relaxed);
        let _refresh = manager.poll();
        fixture
            .inspected
            .recv_timeout(Duration::from_secs(1))
            .expect("mount change starts a bounded refresh");

        assert_eq!(fixture.inspections.load(Ordering::Relaxed), 2);
        assert_eq!(manager.presentations()[0].state, InspectionState::Running);
    }

    #[test]
    fn stale_inspection_result_cannot_replace_refreshed_attachment() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _arrival = manager.reconcile(false, false);
        let stale_generation = manager.attachments[0].generation;
        manager.attachments[0].inspection = Inspection::Running;

        let _refresh = manager.reconcile(false, true);
        let current_generation = manager.attachments[0].generation;
        assert_ne!(stale_generation, current_generation);
        assert!(matches!(
            manager.attachments[0].inspection,
            Inspection::Available
        ));

        manager
            .inspection_tx
            .send(InspectionResult {
                key: "test://mock-cycle".to_owned(),
                generation: stale_generation,
                result: Ok(metadata(7)),
            })
            .expect("the manager retains its inspection receiver");
        assert!(manager.collect_inspections().is_empty());
        assert!(matches!(
            manager.attachments[0].inspection,
            Inspection::Available
        ));

        manager.attachments[0].inspection = Inspection::Running;
        manager
            .inspection_tx
            .send(InspectionResult {
                key: "test://mock-cycle".to_owned(),
                generation: current_generation,
                result: Ok(metadata(43)),
            })
            .expect("the manager retains its inspection receiver");
        assert!(matches!(
            manager.collect_inspections().as_slice(),
            [Event::Inspected { key, .. }] if key == "test://mock-cycle"
        ));
        assert_eq!(
            manager.presentations()[0].identifier,
            Some(DeviceId::from_u32(43))
        );
    }

    #[test]
    fn refresh_keeps_previous_values_and_coalesces_requests() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _arrival = manager.reconcile(false, false);
        manager.attachments[0].inspection = Inspection::Ready(metadata(42));
        manager.refresh("test://mock-cycle").unwrap();
        let generation = manager.attachments[0].generation;
        manager.refresh("test://mock-cycle").unwrap();
        assert_eq!(manager.attachments[0].generation, generation);
        let view = &manager.presentations()[0];
        assert_eq!(view.identifier, Some(DeviceId::from_u32(42)));
        assert_eq!(view.state, InspectionState::Running);
        assert!(manager.refresh("disconnected").is_err());
    }

    #[test]
    fn mutation_requires_a_read_after_the_in_flight_inspection() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _arrival = manager.reconcile(false, false);
        manager.attachments[0].inspection = Inspection::Running;
        manager.invalidate("test://mock-cycle");
        manager.invalidate("test://mock-cycle");
        manager
            .inspection_tx
            .send(InspectionResult {
                key: "test://mock-cycle".to_owned(),
                generation: manager.attachments[0].generation,
                result: Ok(metadata(7)),
            })
            .unwrap();
        assert!(manager.collect_inspections().is_empty());
        assert!(matches!(
            manager.attachments[0].inspection,
            Inspection::Available
        ));
        assert_eq!(manager.presentations()[0].identifier, None);
    }

    #[test]
    fn detached_generation_cannot_enrich_a_reconnected_device() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _arrival = manager.reconcile(false, false);
        let generation = manager.attachments[0].generation;
        let candidate = fixture.candidates.lock().unwrap().pop().unwrap();
        let _departure = manager.reconcile(true, false);
        fixture.candidates.lock().unwrap().push(candidate);
        let _reconnect = manager.reconcile(true, false);
        manager
            .inspection_tx
            .send(InspectionResult {
                key: "test://mock-cycle".to_owned(),
                generation,
                result: Ok(metadata(7)),
            })
            .unwrap();
        assert!(manager.collect_inspections().is_empty());
        assert_eq!(manager.presentations()[0].identifier, None);
    }

    struct Fixture {
        backend: TestBackend,
        candidates: Arc<Mutex<Vec<TestCandidate>>>,
        changed: Arc<AtomicBool>,
        inspections: Arc<AtomicUsize>,
        inspected: mpsc::Receiver<()>,
    }

    fn fixture() -> Fixture {
        let inspections = Arc::new(AtomicUsize::new(0));
        let (inspected_tx, inspected) = mpsc::channel();
        let candidates = Arc::new(Mutex::new(vec![TestCandidate {
            key: "test://mock-cycle",
            inspections: Arc::clone(&inspections),
            inspected: inspected_tx,
        }]));
        let changed = Arc::new(AtomicBool::new(false));
        Fixture {
            backend: TestBackend {
                candidates: Arc::clone(&candidates),
                changed: Arc::clone(&changed),
            },
            candidates,
            changed,
            inspections,
            inspected,
        }
    }

    fn metadata(id: u32) -> Metadata {
        Metadata {
            id: Some(DeviceId::from_u32(id)),
            device_digest: None,
            report: None,
            name: "Mock Cycle-o-Matic 9000".to_owned(),
            software_version: Some(SoftwareVersion::from_hundredths(912)),
            storage: crate::DeviceStateSnapshot::default(),
            capabilities: vec![Capability::new(
                DataType::Activity,
                TransferDirection::OutputFromUnit,
            )],
        }
    }

    fn wait_until_ready(manager: &mut Manager<TestBackend>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while manager.presentations()[0].state != InspectionState::Ready {
            let _events = manager.poll();
            assert!(
                std::time::Instant::now() < deadline,
                "the inspection result reaches the manager"
            );
            std::thread::yield_now();
        }
    }
}
