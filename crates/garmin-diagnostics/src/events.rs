use super::response::{Batch, Cursor};
use garmin_model::diagnostics::{Observation, Update};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

const RECORDS: usize = 2048;
const BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Store>>);

struct Store {
    epoch: uuid::Uuid,
    next: u64,
    lost_through: u64,
    error: Option<String>,
    records: VecDeque<Event>,
    bytes: usize,
    state: BTreeMap<(String, String, String), Event>,
    revisions: BTreeMap<String, u64>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4(),
            next: 0,
            lost_through: 0,
            error: None,
            records: VecDeque::new(),
            bytes: 0,
            state: BTreeMap::new(),
            revisions: BTreeMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Event {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub source: String,
    #[serde(flatten)]
    pub observation: Observation,
}

impl Events {
    /// Publish an owner-observed transition;
    /// identical states do not create events.
    pub fn publish(&self, source: &str, observation: Observation) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(source, observation);
    }

    /// Apply a bounded source update.
    /// A source gap is recorded before replacing its state.
    pub fn update(&self, source: &str, update: Update) {
        let mut store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if store
            .revisions
            .get(source)
            .is_some_and(|revision| *revision >= update.revision)
        {
            return;
        }
        if update.gap {
            // Reserve a cursor boundary even when the lost records do not match a reader's filters.
            store.next += 1;
            store.lost_through = store.next;
        }
        if update.incomplete {
            store.error = Some(format!(
                "Diagnostic state from {source} is incomplete; source limits rejected updates"
            ));
        }
        store.collection_health(
            source,
            if update.incomplete {
                "incomplete"
            } else {
                "ready"
            },
            None,
        );
        for observation in update.changes {
            store.push(source, observation);
        }
        let removed: Vec<_> = store
            .state
            .iter()
            .filter(|((owner, kind, window), _)| {
                owner == source
                    && kind != "connection"
                    && !update
                        .state
                        .iter()
                        .any(|item| &item.kind == kind && &item.window == window)
            })
            .map(|(_, event)| {
                let mut item = event.observation.clone();
                item.removed = true;
                item
            })
            .collect();
        for observation in removed.into_iter().chain(update.state) {
            store.push(source, observation);
        }
        store.revisions.insert(source.into(), update.revision);
    }

    /// Revoke a disconnected source and record removals of its owned state.
    pub fn disconnect(&self, source: &str) {
        let mut store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        store.remove_matching(source, |_| true);
        store.revisions.remove(source);
    }

    /// Invalidate telemetry while the application's connection remains open.
    pub fn unavailable(&self, source: &str, reason: &str) {
        let mut store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        store.remove_matching(source, |item| item.kind != "connection");
        store.revisions.remove(source);
        store.collection_health(source, "unavailable", Some(reason));
    }

    pub(crate) fn read(&self, query: &super::query::Query) -> Batch<Event> {
        let store = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let after = query.after.as_ref();
        let gap = after.is_some_and(|cursor| {
            cursor.epoch != store.epoch
                || cursor.sequence < store.lost_through
                || cursor.sequence > store.next
        });
        let start = if gap {
            None
        } else {
            after.map(|cursor| cursor.sequence)
        };
        let mut candidates: Vec<_> = store
            .records
            .iter()
            .filter(|item| start.is_none_or(|seq| item.sequence > seq) && query.matches(item, true))
            .collect();
        if start.is_none() {
            candidates.reverse();
        }
        let mut records = Vec::new();
        let mut bytes = 0;
        for item in candidates.into_iter().take(query.limit) {
            let size = serde_json::to_vec(item).map_or(0, |data| data.len());
            if bytes + size > 256 * 1024 {
                break;
            }
            bytes += size;
            records.push(item.clone());
        }
        if start.is_none() {
            records.reverse();
        }
        let last = records
            .last()
            .map_or(start.unwrap_or(store.next), |item| item.sequence);
        let more = store
            .records
            .iter()
            .any(|item| item.sequence > last && query.matches(item, true));
        let cursor = if more { last } else { store.next };
        Batch {
            state: Some(
                store
                    .state
                    .values()
                    .filter(|item| query.matches(item, false))
                    .cloned()
                    .collect(),
            ),
            snapshot_revision: Some(Cursor {
                epoch: store.epoch,
                sequence: store.next,
            }),
            records,
            cursor: Cursor {
                epoch: store.epoch,
                sequence: cursor,
            },
            gap,
            more,
            error: store.error.clone(),
        }
    }
}

impl Store {
    fn collection_health(&mut self, source: &str, health: &str, error: Option<&str>) {
        let key = (source.into(), "connection".into(), String::new());
        let Some(event) = self.state.get(&key) else {
            return;
        };
        let mut observation = event.observation.clone();
        observation
            .fields
            .insert("diagnostics".into(), health.into());
        observation.fields.remove("error");
        if let Some(error) = error {
            observation.fields.insert("error".into(), error.into());
        }
        self.push(source, observation);
    }

    fn remove_matching(&mut self, source: &str, matches: impl Fn(&Observation) -> bool) {
        let removed: Vec<_> = self
            .state
            .values()
            .filter(|event| event.source == source && matches(&event.observation))
            .map(|event| event.observation.clone())
            .collect();
        for mut item in removed {
            item.removed = true;
            self.push(source, item);
        }
    }

    fn push(&mut self, source: &str, observation: Observation) {
        if observation.kind == "window" && observation.removed && observation.valid() {
            self.remove_matching(source, |item| {
                item.window == observation.window && item.kind != "window"
            });
        }
        let key = (
            source.to_owned(),
            observation.kind.clone(),
            observation.window.clone(),
        );
        if self
            .state
            .get(&key)
            .is_some_and(|old| old.observation == observation)
            || (observation.removed && !self.state.contains_key(&key))
        {
            return;
        }
        // Bound both retained history and current state,
        // including hostile browser observations.
        if source.len() > 128
            || !observation.valid()
            || (!self.state.contains_key(&key) && self.state.len() >= 128)
        {
            self.error =
                Some("Diagnostic state limit exceeded; some source updates were rejected".into());
            return;
        }
        self.next += 1;
        let event = Event {
            sequence: self.next,
            timestamp_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            source: source.into(),
            observation,
        };
        let size = serde_json::to_vec(&event)
            .expect("diagnostic records contain strings and integers")
            .len();
        if size > 4096 {
            self.error = Some("Diagnostic record exceeds 4096 bytes".into());
            return;
        }
        if event.observation.removed {
            self.state.remove(&key);
        } else {
            self.state.insert(key, event.clone());
        }
        if self
            .records
            .back()
            .is_some_and(|previous| progress_replaces(previous, &event))
            && let Some(previous) = self.records.pop_back()
        {
            self.bytes -= serde_json::to_vec(&previous)
                .expect("valid diagnostic record")
                .len();
        }
        self.bytes += size;
        self.records.push_back(event);
        while self.records.len() > RECORDS || self.bytes > BYTES {
            if let Some(old) = self.records.pop_front() {
                self.lost_through = self.lost_through.max(old.sequence);
                self.bytes -= serde_json::to_vec(&old).map_or(0, |data| data.len());
            }
        }
    }
}

fn progress_replaces(previous: &Event, current: &Event) -> bool {
    previous.source == current.source
        && previous.observation.window == current.observation.window
        && previous.observation.kind == "automation"
        && current.observation.kind == "automation"
        && current
            .observation
            .fields
            .get("state")
            .is_some_and(|value| value == "running")
        && ["state", "scenario", "phase"].iter().all(|key| {
            previous.observation.fields.get(*key) == current.observation.fields.get(*key)
        })
}
