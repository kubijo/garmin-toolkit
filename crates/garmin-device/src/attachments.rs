//! Platform-neutral attached-device lifecycle.

use std::{collections::HashMap, sync::mpsc, time::Duration};

use crate::capabilities::{DataType, DeviceId, SoftwareVersion, TransferDirection};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

/// One host attachment that can be inspected after consent.
pub trait Candidate: Clone + Send + 'static {
    /// Opaque identity for the current attachment.
    fn key(&self) -> &str;

    /// Host-provided display name.
    fn name(&self) -> &str;

    /// Reads and validates device metadata.
    /// # Errors
    /// a stable failure summary when the attachment cannot be inspected.
    fn inspect(&self) -> Result<Metadata, String>;
}

/// Platform attachment-discovery adapter.
pub trait Backend {
    /// Candidate produced by this adapter.
    type Candidate: Candidate;

    /// Dispatches platform events and reports stale discovery state.
    fn poll_changed(&self) -> bool;

    /// The current attachment snapshot without inspecting contents.
    fn candidates(&self) -> Vec<Self::Candidate>;
}

/// Device-list or inspection change consumed by the view.
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
        /// Manifest-provided device name.
        name: String,
    },
    InspectionFailed {
        /// Best available device name.
        name: String,
        /// Stable failure summary.
        reason: String,
    },
}

/// Owned display data for one attachment.
pub struct Presentation {
    pub key: String,
    pub name: String,
    pub identifier: Option<DeviceId>,
    pub software_version: Option<SoftwareVersion>,
    pub capabilities: Vec<Capability>,
    pub storage: Option<crate::DeviceStateSnapshot>,
    pub state: InspectionState,
}

/// One supported device data transfer.
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

/// Consent-gated inspection state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionState {
    Available,
    Running,
    Ready,
    Failed,
}

/// Attachment reconciliation and consented inspection state.
pub struct Manager<B>
where
    B: Backend,
{
    backend: B,
    attachments: Vec<Attachment<B::Candidate>>,
    last_reconcile: Option<std::time::Instant>,
    inspection_tx: mpsc::Sender<InspectionResult>,
    inspection_rx: mpsc::Receiver<InspectionResult>,
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
        }
    }

    /// Dispatches mount events, repairs state periodically, and collects inspections.
    pub fn poll(&mut self) -> Vec<Event> {
        let now = std::time::Instant::now();
        let initial = self.last_reconcile.is_none();
        let due = self
            .last_reconcile
            .is_none_or(|last| now.duration_since(last) >= RECONCILE_INTERVAL);
        let mut events = if self.backend.poll_changed() || due {
            self.last_reconcile = Some(now);
            self.reconcile(!initial)
        } else {
            Vec::new()
        };
        events.extend(self.collect_inspections());
        events
    }

    #[must_use]
    pub fn presentations(&self) -> Vec<Presentation> {
        self.attachments
            .iter()
            .map(|attachment| {
                let (name, identifier, software_version, capabilities, state) =
                    match &attachment.inspection {
                        Inspection::Available => (
                            attachment.name.clone(),
                            None,
                            None,
                            Vec::new(),
                            InspectionState::Available,
                        ),
                        Inspection::Running => (
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

    /// Starts reading and validating one device manifest after consent.
    /// # Errors
    /// An error when no matching mount exists or the inspection worker cannot start.
    pub fn inspect(&mut self, key: &str) -> Result<(), String> {
        let attachment = self
            .attachments
            .iter_mut()
            .find(|attachment| attachment.key == key)
            .ok_or_else(|| "the attached device is no longer available".to_owned())?;
        if matches!(
            attachment.inspection,
            Inspection::Running | Inspection::Ready(_)
        ) {
            return Ok(());
        }

        attachment.inspection = Inspection::Running;
        let candidate = attachment.candidate.clone();
        let key = attachment.key.clone();
        let sender = self.inspection_tx.clone();
        std::thread::Builder::new()
            .name("garmin-toolkit-device-inspection".to_owned())
            .spawn(move || {
                let result = candidate.inspect();
                let _ignored = sender.send(InspectionResult { key, result });
            })
            .map(|_| ())
            .map_err(|error| {
                attachment.inspection = Inspection::Failed;
                format!("could not start device inspection: {error}")
            })
    }

    fn reconcile(&mut self, report_arrivals: bool) -> Vec<Event> {
        let mut previous = std::mem::take(&mut self.attachments)
            .into_iter()
            .map(|attachment| (attachment.key.clone(), attachment))
            .collect::<HashMap<_, _>>();
        let mut events = Vec::new();
        for candidate in self.backend.candidates() {
            let key = candidate.key().to_owned();
            if let Some(mut attachment) = previous.remove(&key) {
                attachment.candidate = candidate;
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
                });
            }
        }
        events.extend(previous.into_keys().map(|key| Event::Detached { key }));
        events
    }

    fn collect_inspections(&mut self) -> Vec<Event> {
        self.inspection_rx
            .try_iter()
            .filter_map(|result| {
                let attachment = self
                    .attachments
                    .iter_mut()
                    .find(|attachment| attachment.key == result.key)?;
                Some(match result.result {
                    Ok(metadata) => {
                        let name = metadata.name.clone();
                        attachment.inspection = Inspection::Ready(metadata);
                        Event::Inspected { name }
                    }
                    Err(reason) => {
                        attachment.inspection = Inspection::Failed;
                        Event::InspectionFailed {
                            name: attachment.name.clone(),
                            reason,
                        }
                    }
                })
            })
            .collect()
    }
}

struct Attachment<C> {
    key: String,
    name: String,
    candidate: C,
    inspection: Inspection,
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
            Ok(Metadata {
                id: DeviceId::from_u32(42),
                name: "Garmin Edge test".to_owned(),
                software_version: SoftwareVersion::from_hundredths(912),
                storage: crate::DeviceStateSnapshot::default(),
                capabilities: vec![Capability::new(
                    DataType::Activity,
                    TransferDirection::OutputFromUnit,
                )],
            })
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
    fn startup_inventory_is_silent_and_does_not_inspect_device_contents() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);

        let events = manager.poll();

        assert!(events.is_empty());
        assert_eq!(fixture.inspections.load(Ordering::Relaxed), 0);
        let presentations = manager.presentations();
        assert_eq!(presentations.len(), 1);
        assert_eq!(presentations[0].key, "test://edge");
        assert_eq!(presentations[0].state, InspectionState::Available);
    }

    #[test]
    fn attachment_after_startup_emits_an_arrival() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _startup = manager.poll();
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
            matches!(events.as_slice(), [Event::Attached { key, .. }] if key == "test://fenix")
        );
    }

    #[test]
    fn explicit_inspection_enriches_the_same_attachment() {
        let fixture = fixture();
        let mut manager = Manager::new(fixture.backend);
        let _events = manager.poll();

        manager
            .inspect("test://edge")
            .expect("the discovered test attachment can be inspected");
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

        assert!(
            matches!(events.as_slice(), [Event::Inspected { name }] if name == "Garmin Edge test")
        );
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
}
