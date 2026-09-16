use std::collections::BTreeMap;

/// Wire-protocol version shared by the browser map worker and its host.
pub const BROWSER_WORKER_PROTOCOL_VERSION: u8 = 1;

/// Kind of work accepted by the browser map worker.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BrowserWorkerTaskKind {
    /// Fetch, decode, style, and tessellate one vector tile.
    Tile,
    /// Shape, place, and tessellate the visible map labels.
    Labels,
    /// Project and segment an activity route.
    Route,
}

impl BrowserWorkerTaskKind {
    /// Stable task name used by the JavaScript wire protocol.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Tile => "tile",
            Self::Labels => "labels",
            Self::Route => "route",
        }
    }

    /// Parse a stable JavaScript wire-protocol task name.
    #[must_use]
    pub fn from_wire_name(value: &str) -> Option<Self> {
        match value {
            "tile" => Some(Self::Tile),
            "labels" => Some(Self::Labels),
            "route" => Some(Self::Route),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowserWorkerPhase {
    Initializing,
    Active,
    Fallback,
}

/// Registration decision for a new browser worker task.
#[derive(Debug, Eq, PartialEq)]
pub enum BrowserWorkerRegistration<T> {
    /// Retain the task until the initialization acknowledgement arrives.
    Queued(u32),
    /// Dispatch the task immediately to the active worker.
    Dispatch(u32),
    /// Execute the task using the local fallback.
    Fallback(T),
}

/// Result of validating a browser worker completion.
#[derive(Debug, Eq, PartialEq)]
pub enum BrowserWorkerCompletion<T> {
    /// The completion matches a live request and may be published.
    Publish(T),
    /// The request was canceled, completed, or belongs to a previous lifecycle.
    Stale,
    /// The response violated the protocol and forced permanent fallback.
    Fallback(Vec<T>),
}

/// Complete effect of a browser worker lifecycle transition.
#[derive(Debug, Eq, PartialEq)]
pub enum BrowserWorkerTransition<T> {
    /// The event did not change the current lifecycle.
    None,
    /// The worker became active and these queued request IDs may be dispatched.
    Dispatch(Vec<u32>),
    /// The worker failed permanently and returns ownership of every live task.
    Fallback(Vec<T>),
}

struct PendingTask<T> {
    kind: BrowserWorkerTaskKind,
    payload: T,
}

/// Payload-owning request ledger and lifecycle for the dedicated browser map worker.
pub struct BrowserWorkerState<T> {
    phase: BrowserWorkerPhase,
    next_id: u32,
    pending: BTreeMap<u32, PendingTask<T>>,
}

impl<T> Default for BrowserWorkerState<T> {
    fn default() -> Self {
        Self {
            phase: BrowserWorkerPhase::Initializing,
            next_id: 0,
            pending: BTreeMap::new(),
        }
    }
}

impl<T> BrowserWorkerState<T> {
    #[cfg(test)]
    #[must_use]
    const fn phase(&self) -> BrowserWorkerPhase {
        self.phase
    }

    /// Register owned work, optionally replacing older work of the same kind.
    pub fn register(
        &mut self,
        kind: BrowserWorkerTaskKind,
        payload: T,
        replace_pending: bool,
    ) -> BrowserWorkerRegistration<T> {
        if self.phase == BrowserWorkerPhase::Fallback {
            return BrowserWorkerRegistration::Fallback(payload);
        }
        if replace_pending {
            self.pending.retain(|_id, task| task.kind != kind);
        }
        let id = self.next_request_id();
        self.pending.insert(id, PendingTask { kind, payload });
        match self.phase {
            BrowserWorkerPhase::Initializing => BrowserWorkerRegistration::Queued(id),
            BrowserWorkerPhase::Active => BrowserWorkerRegistration::Dispatch(id),
            BrowserWorkerPhase::Fallback => unreachable!("fallback returned before registration"),
        }
    }

    /// Accept the worker's initialization acknowledgement.
    pub fn ready(&mut self, version: u8) -> BrowserWorkerTransition<T> {
        if version != BROWSER_WORKER_PROTOCOL_VERSION {
            return self.enter_fallback();
        }
        match self.phase {
            BrowserWorkerPhase::Initializing => {
                self.phase = BrowserWorkerPhase::Active;
                BrowserWorkerTransition::Dispatch(self.pending.keys().copied().collect())
            }
            BrowserWorkerPhase::Active | BrowserWorkerPhase::Fallback => {
                BrowserWorkerTransition::None
            }
        }
    }

    /// Borrow a live task for transport dispatch without transferring ownership.
    #[must_use]
    pub fn task(&self, id: u32) -> Option<&T> {
        self.pending.get(&id).map(|task| &task.payload)
    }

    /// Validate and retire a task completion or task-local error.
    pub fn complete(
        &mut self,
        version: u8,
        id: u32,
        kind: BrowserWorkerTaskKind,
    ) -> BrowserWorkerCompletion<T> {
        if self.phase == BrowserWorkerPhase::Fallback {
            return BrowserWorkerCompletion::Stale;
        }
        if version != BROWSER_WORKER_PROTOCOL_VERSION {
            return BrowserWorkerCompletion::Fallback(self.drain_to_fallback());
        }
        match self.pending.get(&id) {
            Some(expected) if expected.kind == kind => {
                let Some(task) = self.pending.remove(&id) else {
                    return BrowserWorkerCompletion::Stale;
                };
                BrowserWorkerCompletion::Publish(task.payload)
            }
            Some(_) => BrowserWorkerCompletion::Fallback(self.drain_to_fallback()),
            None => BrowserWorkerCompletion::Stale,
        }
    }

    /// Enter permanent fallback after an explicit fatal worker message.
    pub fn fatal(&mut self, _version: u8) -> BrowserWorkerTransition<T> {
        self.enter_fallback()
    }

    /// Enter permanent fallback if initialization has not completed.
    pub fn initialization_timeout(&mut self) -> BrowserWorkerTransition<T> {
        if self.phase == BrowserWorkerPhase::Initializing {
            self.enter_fallback()
        } else {
            BrowserWorkerTransition::None
        }
    }

    /// Enter permanent fallback when a live task stops responding.
    pub fn task_timeout(&mut self, id: u32) -> BrowserWorkerTransition<T> {
        if self.pending.contains_key(&id) {
            self.enter_fallback()
        } else {
            BrowserWorkerTransition::None
        }
    }

    /// Enter permanent fallback after a transport or malformed-message failure.
    pub fn transport_failure(&mut self) -> BrowserWorkerTransition<T> {
        self.enter_fallback()
    }

    fn next_request_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self.pending.contains_key(&self.next_id) {
                return self.next_id;
            }
        }
    }

    fn enter_fallback(&mut self) -> BrowserWorkerTransition<T> {
        if self.phase == BrowserWorkerPhase::Fallback {
            return BrowserWorkerTransition::None;
        }
        BrowserWorkerTransition::Fallback(self.drain_to_fallback())
    }

    fn drain_to_fallback(&mut self) -> Vec<T> {
        self.phase = BrowserWorkerPhase::Fallback;
        std::mem::take(&mut self.pending)
            .into_values()
            .map(|task| task.payload)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_dispatches_queued_tasks_only_for_the_matching_version() {
        let mut state = BrowserWorkerState::default();
        assert_eq!(
            state.register(BrowserWorkerTaskKind::Tile, "tile", false),
            BrowserWorkerRegistration::Queued(1)
        );

        let transition = state.ready(BROWSER_WORKER_PROTOCOL_VERSION);

        assert_eq!(transition, BrowserWorkerTransition::Dispatch(vec![1]));
        assert_eq!(state.task(1), Some(&"tile"));
        assert_eq!(state.phase(), BrowserWorkerPhase::Active);
    }

    #[test]
    fn fatal_initialization_falls_back_every_queued_task() {
        let mut state = BrowserWorkerState::default();
        state.register(BrowserWorkerTaskKind::Tile, "tile", false);
        state.register(BrowserWorkerTaskKind::Labels, "labels", false);

        let transition = state.fatal(BROWSER_WORKER_PROTOCOL_VERSION);

        assert_eq!(
            transition,
            BrowserWorkerTransition::Fallback(vec!["tile", "labels"])
        );
        assert_eq!(state.phase(), BrowserWorkerPhase::Fallback);
        assert_eq!(
            state.register(BrowserWorkerTaskKind::Route, "route", false),
            BrowserWorkerRegistration::Fallback("route")
        );
    }

    #[test]
    fn completion_returns_its_payload_and_keeps_worker_active() {
        let mut state = BrowserWorkerState::default();
        state.ready(BROWSER_WORKER_PROTOCOL_VERSION);
        let BrowserWorkerRegistration::Dispatch(id) =
            state.register(BrowserWorkerTaskKind::Tile, "tile", false)
        else {
            panic!("active worker must dispatch")
        };

        assert_eq!(
            state.complete(
                BROWSER_WORKER_PROTOCOL_VERSION,
                id,
                BrowserWorkerTaskKind::Tile
            ),
            BrowserWorkerCompletion::Publish("tile")
        );
        assert_eq!(state.phase(), BrowserWorkerPhase::Active);
    }

    #[test]
    fn initialization_and_live_task_timeouts_enter_fallback() {
        let mut initializing = BrowserWorkerState::default();
        initializing.register(BrowserWorkerTaskKind::Labels, "labels", false);
        assert_eq!(
            initializing.initialization_timeout(),
            BrowserWorkerTransition::Fallback(vec!["labels"])
        );

        let mut active = BrowserWorkerState::default();
        active.ready(BROWSER_WORKER_PROTOCOL_VERSION);
        let BrowserWorkerRegistration::Dispatch(id) =
            active.register(BrowserWorkerTaskKind::Route, "route", false)
        else {
            panic!("active worker must dispatch")
        };
        assert_eq!(
            active.task_timeout(id),
            BrowserWorkerTransition::Fallback(vec!["route"])
        );
    }

    #[test]
    fn replacement_cancels_the_old_payload_and_makes_its_completion_stale() {
        let mut state = BrowserWorkerState::default();
        state.ready(BROWSER_WORKER_PROTOCOL_VERSION);
        let BrowserWorkerRegistration::Dispatch(old_id) =
            state.register(BrowserWorkerTaskKind::Labels, "old", true)
        else {
            panic!("active worker must dispatch")
        };
        let BrowserWorkerRegistration::Dispatch(new_id) =
            state.register(BrowserWorkerTaskKind::Labels, "new", true)
        else {
            panic!("replacement task must dispatch")
        };

        assert_eq!(
            state.complete(
                BROWSER_WORKER_PROTOCOL_VERSION,
                old_id,
                BrowserWorkerTaskKind::Labels
            ),
            BrowserWorkerCompletion::Stale
        );
        assert_eq!(state.task(new_id), Some(&"new"));
        assert_eq!(state.phase(), BrowserWorkerPhase::Active);
    }

    #[test]
    fn mismatched_task_kind_is_a_fatal_protocol_error() {
        let mut state = BrowserWorkerState::default();
        state.ready(BROWSER_WORKER_PROTOCOL_VERSION);
        let BrowserWorkerRegistration::Dispatch(id) =
            state.register(BrowserWorkerTaskKind::Tile, "tile", false)
        else {
            panic!("active worker must dispatch")
        };

        assert_eq!(
            state.complete(
                BROWSER_WORKER_PROTOCOL_VERSION,
                id,
                BrowserWorkerTaskKind::Route
            ),
            BrowserWorkerCompletion::Fallback(vec!["tile"])
        );
        assert_eq!(state.phase(), BrowserWorkerPhase::Fallback);
    }

    #[test]
    fn mismatched_completion_version_returns_every_payload_exactly_once() {
        let mut state = BrowserWorkerState::default();
        state.ready(BROWSER_WORKER_PROTOCOL_VERSION);
        let BrowserWorkerRegistration::Dispatch(id) =
            state.register(BrowserWorkerTaskKind::Tile, "tile", false)
        else {
            panic!("active worker must dispatch")
        };
        state.register(BrowserWorkerTaskKind::Route, "route", false);

        assert_eq!(
            state.complete(0, id, BrowserWorkerTaskKind::Tile),
            BrowserWorkerCompletion::Fallback(vec!["tile", "route"])
        );
        assert_eq!(state.transport_failure(), BrowserWorkerTransition::None);
    }

    #[test]
    fn transport_failure_returns_every_live_payload_exactly_once() {
        let mut state = BrowserWorkerState::default();
        state.ready(BROWSER_WORKER_PROTOCOL_VERSION);
        state.register(BrowserWorkerTaskKind::Tile, "tile", false);
        state.register(BrowserWorkerTaskKind::Labels, "labels", true);
        state.register(BrowserWorkerTaskKind::Route, "route", true);

        assert_eq!(
            state.transport_failure(),
            BrowserWorkerTransition::Fallback(vec!["tile", "labels", "route"])
        );
        assert_eq!(state.transport_failure(), BrowserWorkerTransition::None);
    }
}
