//! Durable, non-blocking lifecycle control for delegated agents.
//!
//! `spawn_subagent` owns creation only. This module owns every operation after
//! creation so observe/wait/input/cancel/close all see the same state machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use nexa_core::activity::{
    ActivityEvent, ActivityEventKind, ActivityRuntime, ActivitySpec, ActivityState, ActivitySurface,
};
use nexa_core::agent::{AgentEvent, AgentSteeringMessage, CancellationToken};
use nexa_core::error::CoreError;
use serde::Serialize;
use tokio::sync::{mpsc, Notify};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentLifecycleStatus {
    Queued,
    Connected,
    Running,
    WaitingInput,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl SubagentLifecycleStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentLifecycleEventKind {
    Spawned,
    Queued,
    Connected,
    ThinkingDelta,
    ToolStarted,
    Progress,
    OutputDelta,
    InputQueued,
    InputApplied,
    Completed,
    Failed,
    Cancelled,
}

impl SubagentLifecycleEventKind {
    fn activity_kind(self) -> ActivityEventKind {
        match self {
            Self::Spawned | Self::Connected | Self::InputQueued | Self::InputApplied => {
                ActivityEventKind::StateChanged
            }
            Self::Queued | Self::ThinkingDelta | Self::Progress => ActivityEventKind::Progress,
            Self::ToolStarted => ActivityEventKind::CommandStarted,
            Self::OutputDelta => ActivityEventKind::StdoutChunk,
            Self::Completed => ActivityEventKind::Completed,
            Self::Failed => ActivityEventKind::Failed,
            Self::Cancelled => ActivityEventKind::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentWorkerSnapshot {
    pub agent_id: String,
    pub parent_call_id: String,
    pub task: String,
    pub role_id: Option<String>,
    pub role: Option<String>,
    pub status: SubagentLifecycleStatus,
    pub result: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentLifecycleObservation {
    pub worker: SubagentWorkerSnapshot,
    pub cursor: u64,
    pub events: Vec<ActivityEvent>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentLifecycleWaitResult {
    pub worker: SubagentWorkerSnapshot,
    pub timed_out: bool,
}

pub struct RegisterSubagentRequest {
    pub agent_id: String,
    pub parent_call_id: String,
    pub task: String,
    pub role_id: Option<String>,
    pub role: Option<String>,
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
    pub task_run_id: Option<String>,
    pub cancel_token: CancellationToken,
    pub activity_runtime: ActivityRuntime,
    pub event_tx: Option<mpsc::WeakSender<AgentEvent>>,
}

pub struct SubagentWorkerRegistration {
    pub agent_id: String,
    pub cancel_token: CancellationToken,
    pub steering_rx: mpsc::UnboundedReceiver<AgentSteeringMessage>,
    pub events: SubagentEventBridge,
}

struct SubagentWorkerState {
    agent_id: String,
    parent_call_id: String,
    task: String,
    role_id: Option<String>,
    role: Option<String>,
    status: SubagentLifecycleStatus,
    result: Option<serde_json::Value>,
    error_message: Option<String>,
    created_at: String,
    updated_at: String,
    cancel_token: CancellationToken,
    steering_tx: mpsc::UnboundedSender<AgentSteeringMessage>,
    activity_runtime: ActivityRuntime,
    event_tx: Option<mpsc::WeakSender<AgentEvent>>,
    conversation_id: Option<String>,
    turn_id: Option<String>,
    task_run_id: Option<String>,
}

impl SubagentWorkerState {
    fn snapshot(&self) -> SubagentWorkerSnapshot {
        SubagentWorkerSnapshot {
            agent_id: self.agent_id.clone(),
            parent_call_id: self.parent_call_id.clone(),
            task: self.task.clone(),
            role_id: self.role_id.clone(),
            role: self.role.clone(),
            status: self.status,
            result: self.result.clone(),
            error_message: self.error_message.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    fn bridge(&self) -> SubagentEventBridge {
        SubagentEventBridge {
            agent_id: self.agent_id.clone(),
            parent_call_id: self.parent_call_id.clone(),
            task: self.task.clone(),
            role_id: self.role_id.clone(),
            role: self.role.clone(),
            conversation_id: self.conversation_id.clone(),
            turn_id: self.turn_id.clone(),
            task_run_id: self.task_run_id.clone(),
            activity_runtime: self.activity_runtime.clone(),
            event_tx: self.event_tx.clone(),
        }
    }
}

struct SubagentLifecycleInner {
    workers: Mutex<HashMap<String, SubagentWorkerState>>,
    notify: Notify,
}

#[derive(Clone)]
pub struct SubagentLifecycleRuntime {
    inner: Arc<SubagentLifecycleInner>,
}

impl Default for SubagentLifecycleRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(SubagentLifecycleInner {
                workers: Mutex::new(HashMap::new()),
                notify: Notify::new(),
            }),
        }
    }
}

impl SubagentLifecycleRuntime {
    pub fn register(
        &self,
        request: RegisterSubagentRequest,
    ) -> Result<SubagentWorkerRegistration, CoreError> {
        let agent_id = request.agent_id.trim().to_string();
        if agent_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "subagent lifecycle requires a non-empty agent id".into(),
            ));
        }
        let (steering_tx, steering_rx) = mpsc::unbounded_channel();
        let now = Utc::now().to_rfc3339();
        let state = SubagentWorkerState {
            agent_id: agent_id.clone(),
            parent_call_id: request.parent_call_id,
            task: request.task,
            role_id: request.role_id,
            role: request.role,
            status: SubagentLifecycleStatus::Queued,
            result: None,
            error_message: None,
            created_at: now.clone(),
            updated_at: now,
            cancel_token: request.cancel_token.clone(),
            steering_tx,
            activity_runtime: request.activity_runtime,
            event_tx: request.event_tx,
            conversation_id: request.conversation_id,
            turn_id: request.turn_id,
            task_run_id: request.task_run_id,
        };
        let events = state.bridge();
        let mut workers = self.workers()?;
        if workers.contains_key(&agent_id) {
            return Err(CoreError::InvalidInput(format!(
                "Subagent '{agent_id}' already exists"
            )));
        }
        workers.insert(agent_id.clone(), state);
        drop(workers);
        self.inner.notify.notify_waiters();
        Ok(SubagentWorkerRegistration {
            agent_id,
            cancel_token: request.cancel_token,
            steering_rx,
            events,
        })
    }

    pub fn snapshot(&self, agent_id: &str) -> Result<SubagentWorkerSnapshot, CoreError> {
        self.workers()?
            .get(agent_id)
            .map(SubagentWorkerState::snapshot)
            .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))
    }

    pub fn ensure_conversation(
        &self,
        agent_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<(), CoreError> {
        let workers = self.workers()?;
        let worker = workers
            .get(agent_id)
            .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
        if worker.conversation_id.as_deref() != conversation_id {
            // Keep handles opaque across conversations rather than revealing
            // that another chat owns a live worker.
            return Err(CoreError::NotFound(format!("Subagent {agent_id}")));
        }
        Ok(())
    }

    pub fn bridge(&self, agent_id: &str) -> Result<SubagentEventBridge, CoreError> {
        self.workers()?
            .get(agent_id)
            .map(SubagentWorkerState::bridge)
            .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))
    }

    pub fn set_status(
        &self,
        agent_id: &str,
        status: SubagentLifecycleStatus,
    ) -> Result<SubagentWorkerSnapshot, CoreError> {
        let snapshot = {
            let mut workers = self.workers()?;
            let worker = workers
                .get_mut(agent_id)
                .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
            if worker.status.is_terminal() {
                return Ok(worker.snapshot());
            }
            worker.status = status;
            worker.updated_at = Utc::now().to_rfc3339();
            worker.snapshot()
        };
        self.inner.notify.notify_waiters();
        Ok(snapshot)
    }

    pub async fn finish(
        &self,
        agent_id: &str,
        status: SubagentLifecycleStatus,
        result: Option<serde_json::Value>,
        error_message: Option<String>,
    ) -> Result<SubagentWorkerSnapshot, CoreError> {
        if !status.is_terminal() {
            return Err(CoreError::InvalidInput(
                "subagent finish requires a terminal status".into(),
            ));
        }
        let (snapshot, bridge) = {
            let mut workers = self.workers()?;
            let worker = workers
                .get_mut(agent_id)
                .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
            worker.status = status;
            worker.result = result;
            worker.error_message = error_message;
            worker.updated_at = Utc::now().to_rfc3339();
            (worker.snapshot(), worker.bridge())
        };
        self.inner.notify.notify_waiters();
        let event_kind = match status {
            SubagentLifecycleStatus::Completed => SubagentLifecycleEventKind::Completed,
            SubagentLifecycleStatus::Cancelled => SubagentLifecycleEventKind::Cancelled,
            _ => SubagentLifecycleEventKind::Failed,
        };
        bridge
            .finish(
                event_kind,
                serde_json::json!({
                    "status": status,
                    "result": &snapshot.result,
                    "errorMessage": &snapshot.error_message,
                }),
            )
            .await?;
        Ok(snapshot)
    }

    pub fn send_input(
        &self,
        agent_id: &str,
        input: String,
    ) -> Result<SubagentEventBridge, CoreError> {
        let workers = self.workers()?;
        let worker = workers
            .get(agent_id)
            .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
        if worker.status.is_terminal() {
            return Err(CoreError::InvalidInput(format!(
                "Subagent '{agent_id}' is already terminal"
            )));
        }
        worker
            .steering_tx
            .send(AgentSteeringMessage::text(input))
            .map_err(|_| CoreError::Agent(format!("Subagent '{agent_id}' input channel closed")))?;
        Ok(worker.bridge())
    }

    pub fn cancel(&self, agent_id: &str) -> Result<SubagentEventBridge, CoreError> {
        let bridge = {
            let mut workers = self.workers()?;
            let worker = workers
                .get_mut(agent_id)
                .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
            if worker.status.is_terminal() {
                return Ok(worker.bridge());
            }
            worker.status = SubagentLifecycleStatus::Cancelling;
            worker.updated_at = Utc::now().to_rfc3339();
            worker.cancel_token.cancel();
            worker.bridge()
        };
        self.inner.notify.notify_waiters();
        Ok(bridge)
    }

    pub fn close(&self, agent_id: &str) -> Result<SubagentWorkerSnapshot, CoreError> {
        let mut workers = self.workers()?;
        let snapshot = workers
            .get(agent_id)
            .map(SubagentWorkerState::snapshot)
            .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
        if !snapshot.status.is_terminal() {
            return Err(CoreError::InvalidInput(format!(
                "Subagent '{agent_id}' is still active; cancel or wait before closing it"
            )));
        }
        workers.remove(agent_id);
        drop(workers);
        self.inner.notify.notify_waiters();
        Ok(snapshot)
    }

    pub async fn observe(
        &self,
        agent_id: &str,
        after_seq: u64,
        wait_up_to: Duration,
    ) -> Result<SubagentLifecycleObservation, CoreError> {
        let deadline = tokio::time::Instant::now() + wait_up_to;
        let activity_runtime = loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (worker, activity_runtime) = {
                let workers = self.workers()?;
                let worker = workers
                    .get(agent_id)
                    .ok_or_else(|| CoreError::NotFound(format!("Subagent {agent_id}")))?;
                (worker.snapshot(), worker.activity_runtime.clone())
            };
            if activity_runtime.get(agent_id).is_some() {
                break activity_runtime;
            }
            if worker.status.is_terminal() || wait_up_to.is_zero() {
                let timed_out = !worker.status.is_terminal();
                return Ok(SubagentLifecycleObservation {
                    worker,
                    cursor: 0,
                    events: Vec::new(),
                    timed_out,
                });
            }
            if tokio::time::timeout_at(deadline, &mut notified)
                .await
                .is_err()
            {
                return Ok(SubagentLifecycleObservation {
                    worker: self.snapshot(agent_id)?,
                    cursor: 0,
                    events: Vec::new(),
                    timed_out: true,
                });
            }
        };
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let observation = activity_runtime
            .observe(agent_id, after_seq, remaining)
            .await?;
        Ok(SubagentLifecycleObservation {
            worker: self.snapshot(agent_id)?,
            cursor: observation.cursor,
            events: observation.events,
            timed_out: observation.timed_out,
        })
    }

    pub async fn wait(
        &self,
        agent_id: &str,
        wait_up_to: Duration,
    ) -> Result<SubagentLifecycleWaitResult, CoreError> {
        let deadline = tokio::time::Instant::now() + wait_up_to;
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let snapshot = self.snapshot(agent_id)?;
            if snapshot.status.is_terminal() || wait_up_to.is_zero() {
                return Ok(SubagentLifecycleWaitResult {
                    timed_out: !snapshot.status.is_terminal(),
                    worker: snapshot,
                });
            }
            if tokio::time::timeout_at(deadline, &mut notified)
                .await
                .is_err()
            {
                let worker = self.snapshot(agent_id)?;
                return Ok(SubagentLifecycleWaitResult {
                    timed_out: !worker.status.is_terminal(),
                    worker,
                });
            }
        }
    }

    fn workers(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, SubagentWorkerState>>, CoreError> {
        self.inner
            .workers
            .lock()
            .map_err(|_| CoreError::Internal("subagent lifecycle registry lock poisoned".into()))
    }
}

#[derive(Clone)]
pub struct SubagentEventBridge {
    agent_id: String,
    parent_call_id: String,
    task: String,
    role_id: Option<String>,
    role: Option<String>,
    conversation_id: Option<String>,
    turn_id: Option<String>,
    task_run_id: Option<String>,
    activity_runtime: ActivityRuntime,
    event_tx: Option<mpsc::WeakSender<AgentEvent>>,
}

impl SubagentEventBridge {
    pub async fn start(&self) -> Result<(), CoreError> {
        let mut spec = ActivitySpec::new(ActivitySurface::Process, "spawn_subagent")
            .with_activity_id(self.agent_id.clone())
            .with_session_id(self.agent_id.clone());
        if let Some(conversation_id) = self.conversation_id.as_deref() {
            spec = spec.with_conversation_id(conversation_id);
        }
        if let Some(turn_id) = self.turn_id.as_deref() {
            spec = spec.with_turn_id(turn_id);
        }
        if let Some(task_run_id) = self.task_run_id.as_deref() {
            spec = spec.with_task_run_id(task_run_id);
        }
        let runtime = self.activity_runtime.clone();
        tokio::task::spawn_blocking(move || runtime.start(spec))
            .await
            .map_err(|error| {
                CoreError::Internal(format!("subagent activity start join: {error}"))
            })??;
        self.emit(
            SubagentLifecycleEventKind::Spawned,
            serde_json::json!({
                "task": self.task,
                "roleId": self.role_id,
                "role": self.role,
            }),
        )
        .await?;
        self.emit(
            SubagentLifecycleEventKind::Queued,
            serde_json::json!({ "status": "queued" }),
        )
        .await?;
        Ok(())
    }

    pub async fn emit(
        &self,
        kind: SubagentLifecycleEventKind,
        detail: serde_json::Value,
    ) -> Result<ActivityEvent, CoreError> {
        let payload = lifecycle_payload(kind, &self.agent_id, detail);
        let runtime = self.activity_runtime.clone();
        let activity_id = self.agent_id.clone();
        let event = tokio::task::spawn_blocking(move || {
            runtime.append(&activity_id, kind.activity_kind(), payload)
        })
        .await
        .map_err(|error| CoreError::Internal(format!("subagent event join: {error}")))??;
        self.forward(kind, event.clone()).await;
        Ok(event)
    }

    pub async fn finish(
        &self,
        kind: SubagentLifecycleEventKind,
        detail: serde_json::Value,
    ) -> Result<ActivityEvent, CoreError> {
        let state = match kind {
            SubagentLifecycleEventKind::Completed => ActivityState::Completed,
            SubagentLifecycleEventKind::Cancelled => ActivityState::Cancelled,
            _ => ActivityState::Failed,
        };
        let payload = lifecycle_payload(kind, &self.agent_id, detail);
        let runtime = self.activity_runtime.clone();
        let activity_id = self.agent_id.clone();
        let event =
            tokio::task::spawn_blocking(move || runtime.transition(&activity_id, state, payload))
                .await
                .map_err(|error| CoreError::Internal(format!("subagent finish join: {error}")))??;
        self.forward(kind, event.clone()).await;
        Ok(event)
    }

    async fn forward(&self, kind: SubagentLifecycleEventKind, event: ActivityEvent) {
        // Detached workers must not keep the parent turn's event stream open.
        // Upgrade only while the parent owns a strong sender; durable Activity
        // events remain observable after the live stream closes.
        let Some(event_tx) = self.event_tx.as_ref().and_then(mpsc::WeakSender::upgrade) else {
            return;
        };
        let note = format!("Subagent {}: {:?}", self.agent_id, kind);
        let _ = event_tx
            .send(AgentEvent::ToolCallProgress {
                call_id: self.parent_call_id.clone(),
                tool_name: "spawn_subagent".to_string(),
                note,
                activity: Some(event),
            })
            .await;
    }
}

fn lifecycle_payload(
    kind: SubagentLifecycleEventKind,
    agent_id: &str,
    detail: serde_json::Value,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "subagentEvent": kind,
        "agentId": agent_id,
        "detail": detail,
    });
    if kind == SubagentLifecycleEventKind::OutputDelta {
        if let Some(data) = payload["detail"]["delta"].as_str() {
            payload["data"] = serde_json::Value::String(data.to_string());
        }
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(runtime: ActivityRuntime) -> RegisterSubagentRequest {
        RegisterSubagentRequest {
            agent_id: "agent-1".into(),
            parent_call_id: "call-1".into(),
            task: "check evidence".into(),
            role_id: Some("verifier".into()),
            role: None,
            conversation_id: None,
            turn_id: None,
            task_run_id: None,
            cancel_token: CancellationToken::new(),
            activity_runtime: runtime,
            event_tx: None,
        }
    }

    #[tokio::test]
    async fn lifecycle_handle_is_observable_controllable_and_closes_only_when_terminal() {
        let lifecycle = SubagentLifecycleRuntime::default();
        let activity = ActivityRuntime::new();
        let registration = lifecycle.register(request(activity)).unwrap();
        assert_eq!(
            lifecycle.snapshot("agent-1").unwrap().status,
            SubagentLifecycleStatus::Queued
        );
        assert!(lifecycle.close("agent-1").is_err());
        let wait = lifecycle.wait("agent-1", Duration::ZERO).await.unwrap();
        assert!(wait.timed_out);
        assert_eq!(wait.worker.status, SubagentLifecycleStatus::Queued);

        registration.events.start().await.unwrap();
        lifecycle
            .set_status("agent-1", SubagentLifecycleStatus::Connected)
            .unwrap();
        registration
            .events
            .emit(
                SubagentLifecycleEventKind::OutputDelta,
                serde_json::json!({ "delta": "evidence" }),
            )
            .await
            .unwrap();
        lifecycle
            .finish(
                "agent-1",
                SubagentLifecycleStatus::Completed,
                Some(serde_json::json!({ "answer": "ok" })),
                None,
            )
            .await
            .unwrap();

        let observation = lifecycle
            .observe("agent-1", 0, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(
            observation.worker.status,
            SubagentLifecycleStatus::Completed
        );
        assert!(observation.events.iter().any(|event| {
            event.payload["subagentEvent"] == serde_json::json!("outputDelta")
                && event.payload["data"] == serde_json::json!("evidence")
        }));
        assert_eq!(
            lifecycle.close("agent-1").unwrap().status,
            SubagentLifecycleStatus::Completed
        );
        assert!(lifecycle.snapshot("agent-1").is_err());
    }

    #[tokio::test]
    async fn cancellation_is_visible_before_worker_settles() {
        let lifecycle = SubagentLifecycleRuntime::default();
        let activity = ActivityRuntime::new();
        let registration = lifecycle.register(request(activity)).unwrap();
        lifecycle.cancel("agent-1").unwrap();
        assert!(registration.cancel_token.is_cancelled());
        assert_eq!(
            lifecycle.snapshot("agent-1").unwrap().status,
            SubagentLifecycleStatus::Cancelling
        );
    }

    #[tokio::test]
    async fn observe_waits_for_the_worker_activity_to_start() {
        let lifecycle = SubagentLifecycleRuntime::default();
        let registration = lifecycle.register(request(ActivityRuntime::new())).unwrap();
        let observer = lifecycle.clone();
        let observation = tokio::spawn(async move {
            observer
                .observe("agent-1", 0, Duration::from_millis(250))
                .await
                .unwrap()
        });

        tokio::task::yield_now().await;
        registration.events.start().await.unwrap();
        lifecycle
            .set_status("agent-1", SubagentLifecycleStatus::Running)
            .unwrap();

        let observation = observation.await.unwrap();
        assert!(!observation.timed_out);
        assert!(observation
            .events
            .iter()
            .any(|event| { event.payload["subagentEvent"] == serde_json::json!("spawned") }));
    }

    #[tokio::test]
    async fn detached_lifecycle_does_not_keep_parent_event_stream_open() {
        let lifecycle = SubagentLifecycleRuntime::default();
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let mut request = request(ActivityRuntime::new());
        request.event_tx = Some(event_tx.downgrade());

        let registration = lifecycle.register(request).unwrap();
        drop(event_tx);

        assert!(event_rx.recv().await.is_none());
        assert_eq!(
            lifecycle.snapshot("agent-1").unwrap().status,
            SubagentLifecycleStatus::Queued
        );
        drop(registration);
    }

    #[test]
    fn lifecycle_handles_remain_scoped_to_their_parent_conversation() {
        let lifecycle = SubagentLifecycleRuntime::default();
        let mut request = request(ActivityRuntime::new());
        request.conversation_id = Some("conversation-a".into());
        let registration = lifecycle.register(request).unwrap();

        assert!(lifecycle
            .ensure_conversation("agent-1", Some("conversation-a"))
            .is_ok());
        assert!(lifecycle
            .ensure_conversation("agent-1", Some("conversation-b"))
            .is_err());
        drop(registration);
    }
}
