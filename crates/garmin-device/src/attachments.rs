//! Platform-neutral attached-device lifecycle.

use std::{collections::HashMap, sync::mpsc, time::Duration};

use crate::capabilities::{DataType, DeviceId, SoftwareVersion, TransferDirection};

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
}

impl<B> Manager<B>
where
    B: Backend,
{
    #[must_use]
    pub fn new(backend: B) -> Self {
        let (inspection_tx, inspection_rx) = mpsc::channel();
        Self {
            backend,
            attachments: Vec::new(),
            last_reconcile: None,
            inspection_tx,
            inspection_rx,
            next_generation: 0,
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
                let (name, identifier, software_version, capabilities, state) =
                    match &attachment.inspection {
                        Inspection::Available | Inspection::Running => (
                            attachment.name.clone(),
                            None,
                            None,
                            Vec::new(),
                            InspectionState::Running,
                        ),
                        Inspection::Ready(metadata) => (
                            metadata.name.clone(),
                            Some(metadata.id),
                            Some(metadata.software_version),
                            metadata.capabilities.clone(),
                            InspectionState::Ready,
                        ),
                        Inspection::Failed => (
                            attachment.name.clone(),
                            None,
                            None,
                            Vec::new(),
                            InspectionState::Failed,
                        ),
                    };
                Presentation {
                    key: attachment.key.clone(),
                    name,
                    identifier,
                    software_version,
                    capabilities,
                    storage: match &attachment.inspection {
                        Inspection::Ready(metadata) => Some(metadata.storage.clone()),
                        _ => None,
                    },
                    state,
                }
            })
            .collect()
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
                    attachment.inspection = Inspection::Available;
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
            if let Err(error) = std::thread::Builder::new()
                .name("garmin-toolkit-device-inspection".to_owned())
                .spawn(move || {
                    let result = candidate.inspect();
                    let _ignored = sender.send(InspectionResult {
                        key,
                        generation,
                        result,
                    });
                })
            {
                let reason = format!("could not start device inspection: {error}");
                attachment.inspection = Inspection::Failed;
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
                    Some(match result {
                        Ok(metadata) => {
                            let name = metadata.name.clone();
                            attachment.inspection = Inspection::Ready(metadata);
                            Event::Inspected { key, name }
                        }
                        Err(reason) => {
                            attachment.inspection = Inspection::Failed;
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
    generation: u64,
}

enum Inspection {
    Available,
    Running,
    Ready(Metadata),
    Failed,
}

/// Validated metadata produced by a target adapter.
pub struct Metadata {
    pub id: DeviceId,
    pub name: String,
    pub software_version: SoftwareVersion,
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
            "Garmin test device"
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
        assert_eq!(presentations[0].key, "test://edge");
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
        let mut fenix = fixture
            .candidates
            .lock()
            .expect("the test retains no poisoned lock")[0]
            .clone();
        fenix.key = "test://fenix";
        fixture
            .candidates
            .lock()
            .expect("the test retains no poisoned lock")
            .push(fenix);
        fixture.changed.store(true, Ordering::Relaxed);

        let events = manager.poll();

        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Attached { key, .. } if key == "test://fenix"))
        );
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
                if key == "test://edge" && name == "Garmin Edge test"));
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
                key: "test://edge".to_owned(),
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
                key: "test://edge".to_owned(),
                generation: current_generation,
                result: Ok(metadata(43)),
            })
            .expect("the manager retains its inspection receiver");
        assert!(matches!(
            manager.collect_inspections().as_slice(),
            [Event::Inspected { key, .. }] if key == "test://edge"
        ));
        assert_eq!(
            manager.presentations()[0].identifier,
            Some(DeviceId::from_u32(43))
        );
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
            key: "test://edge",
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
            id: DeviceId::from_u32(id),
            name: "Garmin Edge test".to_owned(),
            software_version: SoftwareVersion::from_hundredths(912),
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
