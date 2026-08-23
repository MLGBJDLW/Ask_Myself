use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Notify};

use super::event_log::{ActivityEntry, DEFAULT_MAX_EVENTS_PER_ACTIVITY};
use super::persistence::{load_entries, persist_event, persist_record};
use super::{ActivityEvent, ActivityEventKind, ActivityRecord, ActivitySpec, ActivityState};
use crate::db::Database;
use crate::error::CoreError;

pub const MAX_OBSERVE_QUANTUM: Duration = Duration::from_millis(2_500);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityObservation {
    pub record: ActivityRecord,
    pub cursor: u64,
    pub events: Vec<ActivityEvent>,
    pub timed_out: bool,
}

struct ActivityRuntimeInner {
    entries: Mutex<HashMap<String, ActivityEntry>>,
    notify: Notify,
    events: broadcast::Sender<ActivityEvent>,
    database: Option<Database>,
    max_events_per_activity: usize,
}

fn persistent_runtimes() -> &'static Mutex<HashMap<PathBuf, Weak<ActivityRuntimeInner>>> {
    static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Weak<ActivityRuntimeInner>>>> =
        OnceLock::new();
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn persistent_runtime_key(database: &Database) -> Option<PathBuf> {
    database.db_path().map(|path| {
        std::fs::canonicalize(path).unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        })
    })
}

#[derive(Clone)]
pub struct ActivityRuntime {
    inner: Arc<ActivityRuntimeInner>,
}

impl Default for ActivityRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityRuntime {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(512);
        Self {
            inner: Arc::new(ActivityRuntimeInner {
                entries: Mutex::new(HashMap::new()),
                notify: Notify::new(),
                events,
                database: None,
                max_events_per_activity: DEFAULT_MAX_EVENTS_PER_ACTIVITY,
            }),
        }
    }

    pub fn is_persistent(&self) -> bool {
        self.inner.database.is_some()
    }

    pub fn with_database(database: Database) -> Result<Self, CoreError> {
        let runtime_key = persistent_runtime_key(&database);
        let mut runtime_cache = runtime_key
            .as_ref()
            .map(|_| match persistent_runtimes().lock() {
                Ok(cache) => cache,
                Err(poisoned) => {
                    tracing::error!(
                        "Persistent Activity Runtime cache was poisoned; recovering inner state"
                    );
                    poisoned.into_inner()
                }
            });
        if let (Some(key), Some(cache)) = (runtime_key.as_ref(), runtime_cache.as_ref()) {
            if let Some(inner) = cache.get(key).and_then(Weak::upgrade) {
                return Ok(Self { inner });
            }
        }

        let entries = load_entries(&database)?;
        let (events, _) = broadcast::channel(512);
        let runtime = Self {
            inner: Arc::new(ActivityRuntimeInner {
                entries: Mutex::new(entries),
                notify: Notify::new(),
                events,
                database: Some(database),
                max_events_per_activity: DEFAULT_MAX_EVENTS_PER_ACTIVITY,
            }),
        };
        runtime.mark_unfinished_as_orphaned()?;
        if let (Some(key), Some(cache)) = (runtime_key, runtime_cache.as_mut()) {
            cache.insert(key, Arc::downgrade(&runtime.inner));
        }
        Ok(runtime)
    }

    pub fn start(&self, spec: ActivitySpec) -> Result<ActivityRecord, CoreError> {
        if spec.activity_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "activity_id cannot be empty".to_string(),
            ));
        }
        let now = Utc::now();
        let mut record = ActivityRecord::from_spec(spec, now);
        let event = ActivityEvent {
            activity_id: record.activity_id.clone(),
            seq: 1,
            timestamp: now,
            kind: ActivityEventKind::Started,
            payload: serde_json::json!({
                "state": record.state,
                "surface": record.surface,
                "ownerTool": record.owner_tool,
                "sessionId": record.session_id,
                "conversationId": record.conversation_id,
                "turnId": record.turn_id,
                "taskRunId": record.task_run_id,
            }),
        };
        let mut entry = ActivityEntry::new(record.clone());
        entry.push(event.clone(), self.inner.max_events_per_activity);
        record = entry.record.clone();
        {
            let mut entries = self.entries();
            if entries.contains_key(&record.activity_id) {
                return Err(CoreError::InvalidInput(format!(
                    "Activity '{}' already exists.",
                    record.activity_id
                )));
            }
            entries.insert(record.activity_id.clone(), entry);
            self.persist(&record, &event)?;
        }
        let _ = self.inner.events.send(event);
        self.inner.notify.notify_waiters();
        tracing::info!(
            target: "activity.runtime",
            activity_id = %record.activity_id,
            surface = ?record.surface,
            state = ?record.state,
            owner_tool = %record.owner_tool,
            cursor = record.last_event_seq,
            "activity started"
        );
        Ok(record)
    }

    pub fn append(
        &self,
        activity_id: &str,
        kind: ActivityEventKind,
        payload: serde_json::Value,
    ) -> Result<ActivityEvent, CoreError> {
        let (record, event) = {
            let mut entries = self.entries();
            let entry = entries
                .get_mut(activity_id)
                .ok_or_else(|| CoreError::NotFound(format!("Activity {activity_id}")))?;
            if entry.record.state.is_terminal() {
                return Err(CoreError::InvalidInput(format!(
                    "Activity '{activity_id}' is already {:?}.",
                    entry.record.state
                )));
            }
            let event = ActivityEvent {
                activity_id: activity_id.to_string(),
                seq: entry.record.last_event_seq.saturating_add(1),
                timestamp: Utc::now(),
                kind,
                payload,
            };
            entry.push(event.clone(), self.inner.max_events_per_activity);
            let record = entry.record.clone();
            self.persist(&record, &event)?;
            (record, event)
        };
        let _ = self.inner.events.send(event.clone());
        self.inner.notify.notify_waiters();
        tracing::debug!(
            target: "activity.runtime",
            activity_id = %record.activity_id,
            event_kind = ?event.kind,
            cursor = event.seq,
            state = ?record.state,
            "activity event appended"
        );
        Ok(event)
    }

    pub fn transition(
        &self,
        activity_id: &str,
        state: ActivityState,
        payload: serde_json::Value,
    ) -> Result<ActivityEvent, CoreError> {
        let (record, event) = {
            let mut entries = self.entries();
            let entry = entries
                .get_mut(activity_id)
                .ok_or_else(|| CoreError::NotFound(format!("Activity {activity_id}")))?;
            if entry.record.state.is_terminal() {
                return Err(CoreError::InvalidInput(format!(
                    "Activity '{activity_id}' is already {:?}.",
                    entry.record.state
                )));
            }
            let now = Utc::now();
            entry.record.state = state;
            entry.record.completed_at = state.is_terminal().then_some(now);
            let kind = match state {
                ActivityState::Completed => ActivityEventKind::Completed,
                ActivityState::Failed => ActivityEventKind::Failed,
                ActivityState::Cancelled => ActivityEventKind::Cancelled,
                ActivityState::Superseded => ActivityEventKind::Superseded,
                ActivityState::TimedOut => ActivityEventKind::TimedOut,
                _ => ActivityEventKind::StateChanged,
            };
            let event = ActivityEvent {
                activity_id: activity_id.to_string(),
                seq: entry.record.last_event_seq.saturating_add(1),
                timestamp: now,
                kind,
                payload: serde_json::json!({
                    "state": state,
                    "detail": payload,
                }),
            };
            entry.push(event.clone(), self.inner.max_events_per_activity);
            let record = entry.record.clone();
            self.persist(&record, &event)?;
            (record, event)
        };
        let _ = self.inner.events.send(event.clone());
        self.inner.notify.notify_waiters();
        tracing::info!(
            target: "activity.runtime",
            activity_id = %record.activity_id,
            state = ?record.state,
            cursor = event.seq,
            elapsed_ms = (record.updated_at - record.started_at).num_milliseconds(),
            "activity state changed"
        );
        Ok(event)
    }

    pub fn get(&self, activity_id: &str) -> Option<ActivityRecord> {
        self.entries()
            .get(activity_id)
            .map(|entry| entry.record.clone())
    }

    pub fn list(&self) -> Vec<ActivityRecord> {
        let mut records = self
            .entries()
            .values()
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.started_at);
        records
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ActivityEvent> {
        self.inner.events.subscribe()
    }

    pub async fn observe(
        &self,
        activity_id: &str,
        after_seq: u64,
        wait_up_to: Duration,
    ) -> Result<ActivityObservation, CoreError> {
        let wait_up_to = wait_up_to.min(MAX_OBSERVE_QUANTUM);
        let deadline = tokio::time::Instant::now() + wait_up_to;
        loop {
            let notified = self.inner.notify.notified();
            let observation = self.snapshot(activity_id, after_seq, false)?;
            if !observation.events.is_empty()
                || observation.record.state.is_terminal()
                || wait_up_to.is_zero()
            {
                return Ok(observation);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, notified).await.is_err() {
                return self.snapshot(activity_id, after_seq, true);
            }
        }
    }

    fn snapshot(
        &self,
        activity_id: &str,
        after_seq: u64,
        timed_out: bool,
    ) -> Result<ActivityObservation, CoreError> {
        let entries = self.entries();
        let entry = entries
            .get(activity_id)
            .ok_or_else(|| CoreError::NotFound(format!("Activity {activity_id}")))?;
        Ok(ActivityObservation {
            record: entry.record.clone(),
            cursor: entry.record.last_event_seq,
            events: entry.events_after(after_seq),
            timed_out,
        })
    }

    fn mark_unfinished_as_orphaned(&self) -> Result<(), CoreError> {
        let activity_ids = self
            .list()
            .into_iter()
            .filter(|record| !record.state.is_terminal())
            .map(|record| record.activity_id)
            .collect::<Vec<_>>();
        for activity_id in activity_ids {
            self.transition(
                &activity_id,
                ActivityState::Orphaned,
                serde_json::json!({
                    "reason": "application_restarted_without_a_recoverable_adapter",
                }),
            )?;
        }
        Ok(())
    }

    fn persist(&self, record: &ActivityRecord, event: &ActivityEvent) -> Result<(), CoreError> {
        if let Some(database) = &self.inner.database {
            persist_record(database, record)?;
            persist_event(database, event)?;
        }
        Ok(())
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, ActivityEntry>> {
        match self.inner.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => {
                tracing::error!("Activity Runtime mutex was poisoned; recovering inner state");
                poisoned.into_inner()
            }
        }
    }
}
