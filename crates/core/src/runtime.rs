//! Host-agnostic Agent Session and Runtime Protocol contracts.
//!
//! Desktop, CLI, protocol exits, and task runners should depend on this
//! interface instead of recreating agent turn assembly in each host surface.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent::power_mode::AgentPowerMode;
use crate::agent::{AgentExecutionMode, AgentSteeringMessage, CancellationToken};
use crate::agent_run::{AgentRunEvent, AGENT_RUN_EVENT_VERSION};
use crate::app_settings::ShellAccessMode;
use crate::approval::{ApprovalDecision, ToolApprovalMode};
use crate::context_maintenance::StartContextCompactionRequest;
use crate::conversation::ImageAttachment;
use crate::error::CoreError;
use crate::mixture_of_agents::{AgentCollaborationMode, MoaPresetId};
use crate::quality_profile::{CustomOrchestrationOptions, OrchestrationProfile};

pub const RUNTIME_PROTOCOL_VERSION: u16 = 1;
const AGENT_RUN_EVENT_OUTBOX_CAPACITY: usize = 512;

/// The only producer interface for one run's ordered event ledger.
#[derive(Clone)]
pub struct AgentRunEventOutbox {
    sender: tokio::sync::mpsc::Sender<AgentRunEvent>,
    failure_handle: AgentRunEventOutboxFailureHandle,
}

/// Sender-free control plane retained by the outbox actor. Keeping this handle
/// alive cannot keep the producer channel open.
#[derive(Clone)]
pub struct AgentRunEventOutboxFailureHandle {
    terminal_submitted: Arc<Mutex<bool>>,
    cancellation: CancellationToken,
}

impl AgentRunEventOutboxFailureHandle {
    pub fn fail_closed(&self) {
        if let Ok(mut terminal_submitted) = self.terminal_submitted.lock() {
            *terminal_submitted = true;
        }
        self.cancellation.cancel();
    }
}

impl AgentRunEventOutbox {
    pub fn channel() -> (Self, tokio::sync::mpsc::Receiver<AgentRunEvent>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(AGENT_RUN_EVENT_OUTBOX_CAPACITY);
        let failure_handle = AgentRunEventOutboxFailureHandle {
            terminal_submitted: Arc::new(Mutex::new(false)),
            cancellation: CancellationToken::new(),
        };
        (
            Self {
                sender,
                failure_handle,
            },
            receiver,
        )
    }

    pub fn submit(&self, event: AgentRunEvent) -> Result<(), CoreError> {
        if event.event_seq != 0 {
            return Err(CoreError::InvalidInput(
                "Run Event producers must submit an unsequenced event".to_string(),
            ));
        }
        let mut terminal_submitted =
            self.failure_handle.terminal_submitted.lock().map_err(|_| {
                CoreError::Internal("Run Event outbox state is poisoned".to_string())
            })?;
        if *terminal_submitted {
            return Err(CoreError::InvalidInput(
                "Run Event outbox is closed after its terminal event".to_string(),
            ));
        }
        let terminal = event.is_terminal();
        if let Err(error) = self.sender.try_send(event) {
            *terminal_submitted = true;
            self.failure_handle.cancellation.cancel();
            return Err(match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => CoreError::InvalidInput(
                    "Run Event outbox capacity was exhausted; the run was cancelled before dropping ordered events"
                        .to_string(),
                ),
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    CoreError::Internal("Run Event outbox actor is unavailable".to_string())
                }
            });
        }
        *terminal_submitted = terminal;
        Ok(())
    }

    pub fn is_closed_for_submission(&self) -> bool {
        self.failure_handle
            .terminal_submitted
            .lock()
            .map(|terminal_submitted| *terminal_submitted)
            .unwrap_or(true)
    }

    pub fn failure_handle(&self) -> AgentRunEventOutboxFailureHandle {
        self.failure_handle.clone()
    }

    /// One executor-scoped child of the run lifetime. Cancelling a suspended
    /// executor does not poison a later continuation, while fail-closed outbox
    /// cancellation reaches every child for this run.
    pub fn turn_cancellation_token(&self) -> CancellationToken {
        self.failure_handle.cancellation.child_token()
    }
}

/// Runtime-owned control plane for one active turn.
///
/// The host owns transport-only concerns; cancellation, steering, task
/// lifetime, identifiers, and event ordering stay together here.
pub struct ActiveAgentTurn {
    pub handle: AgentTurnHandle,
    pub cancel_token: CancellationToken,
    pub task: tokio::task::JoinHandle<()>,
    pub steering_tx: tokio::sync::mpsc::UnboundedSender<AgentSteeringMessage>,
    pub event_outbox: Arc<AgentRunEventOutbox>,
    pub orchestrator_run_id: Option<String>,
    pub frontend_paint_recorded: AtomicBool,
}

impl ActiveAgentTurn {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub fn steer(&self, message: impl Into<String>) -> Result<(), CoreError> {
        if self.is_finished() {
            return Err(CoreError::InvalidInput(
                "Agent turn is no longer running.".to_string(),
            ));
        }
        self.steering_tx
            .send(AgentSteeringMessage::text(message.into()))
            .map_err(|_| {
                CoreError::InvalidInput(
                    "Agent turn is no longer accepting steering messages.".to_string(),
                )
            })
    }

    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    pub fn abort(&self) {
        self.task.abort();
    }
}

/// Owns all live turns for a host process.
///
/// Registering a second turn for one session cancels and aborts the previous
/// turn before replacing it, so every host observes the same lifecycle rule.
#[derive(Default)]
pub struct AgentSessionManager {
    active: tokio::sync::Mutex<HashMap<String, ActiveAgentTurn>>,
}

impl AgentSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, turn: ActiveAgentTurn) {
        let session_id = turn.handle.session_id.clone();
        let mut active = self.active.lock().await;
        if let Some(previous) = active.remove(&session_id) {
            previous.cancel();
            previous.abort();
        }
        active.insert(session_id, turn);
    }

    pub async fn steer(
        &self,
        session_id: &str,
        message: impl Into<String>,
    ) -> Result<(), CoreError> {
        let mut active = self.active.lock().await;
        let Some(turn) = active.get(session_id) else {
            return Err(CoreError::NotFound(
                "No running agent turn for this session.".to_string(),
            ));
        };
        if turn.is_finished() {
            if turn.event_outbox.is_closed_for_submission() {
                active.remove(session_id);
            }
            return Err(CoreError::NotFound(
                "No running agent turn for this session.".to_string(),
            ));
        }
        turn.steer(message)
    }

    pub async fn take(&self, session_id: &str) -> Option<ActiveAgentTurn> {
        let turn = self.active.lock().await.remove(session_id)?;
        (!turn.is_finished() || !turn.event_outbox.is_closed_for_submission()).then_some(turn)
    }

    pub async fn take_for_run(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> Result<Option<ActiveAgentTurn>, CoreError> {
        let mut active = self.active.lock().await;
        let Some(turn) = active.get(session_id) else {
            return Ok(None);
        };
        if turn.handle.run_id != run_id {
            return Err(CoreError::InvalidInput(format!(
                "Interaction continuation run mismatch: active {}, resumed {run_id}",
                turn.handle.run_id
            )));
        }
        let turn = active
            .remove(session_id)
            .expect("validated agent session should remain registered");
        Ok((!turn.is_finished() || !turn.event_outbox.is_closed_for_submission()).then_some(turn))
    }

    pub async fn contains(&self, session_id: &str) -> bool {
        let mut active = self.active.lock().await;
        if let Some(turn) = active.get(session_id) {
            if turn.is_finished() {
                if turn.event_outbox.is_closed_for_submission() {
                    active.remove(session_id);
                }
                return false;
            }
        } else {
            return false;
        }
        true
    }

    /// Claim the one frontend-paint metric allowed for an active turn.
    ///
    /// The identity check prevents a delayed frame from a previous turn from
    /// writing telemetry into a newer turn for the same conversation.
    pub async fn claim_frontend_paint_metric(
        &self,
        session_id: &str,
        run_id: &str,
        turn_id: &str,
    ) -> bool {
        let active = self.active.lock().await;
        let Some(turn) = active.get(session_id) else {
            return false;
        };
        if turn.handle.run_id != run_id || turn.handle.turn_id != turn_id {
            return false;
        }
        if turn.frontend_paint_recorded.swap(true, Ordering::SeqCst) {
            return false;
        }
        true
    }

    /// Returns `None` only while another runtime operation holds the manager.
    pub fn try_is_empty(&self) -> Option<bool> {
        self.active.try_lock().ok().map(|mut active| {
            active.retain(|_, turn| {
                !turn.is_finished() || !turn.event_outbox.is_closed_for_submission()
            });
            !active.values().any(|turn| !turn.is_finished())
        })
    }
}

/// Canonical host request for starting one agent turn.
///
/// Hosts may add transport-specific metadata before handing this request to a
/// runtime adapter, but the identifiers and execution policy enter through one
/// versioned boundary. `idempotency_key` is supplied by the caller so retrying
/// an uncertain launch cannot create a second user message or run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTurnRequest {
    #[serde(default = "runtime_protocol_version")]
    pub version: u16,
    #[serde(default = "new_runtime_id")]
    pub idempotency_key: String,
    pub conversation_id: String,
    pub message: String,
    #[serde(default)]
    pub attachments: Vec<ImageAttachment>,
    #[serde(default)]
    pub agent_config_id: Option<String>,
    #[serde(default)]
    pub persona_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub execution_mode: AgentExecutionMode,
    #[serde(default)]
    pub power_mode: AgentPowerMode,
    #[serde(default)]
    pub collaboration_mode: AgentCollaborationMode,
    #[serde(default)]
    pub moa_preset: MoaPresetId,
    #[serde(default)]
    pub orchestration_profile: OrchestrationProfile,
    #[serde(default)]
    pub custom_orchestration: Option<CustomOrchestrationOptions>,
    #[serde(default)]
    pub vision_turn_override: Option<crate::vision_router::VisionTurnOverride>,
    #[serde(default)]
    pub user_artifacts: Option<serde_json::Value>,
    #[serde(default)]
    pub task_orchestrator_run_id: Option<String>,
}

impl StartTurnRequest {
    pub fn apply_protocol_defaults(&mut self) {
        if self.version == 0 {
            self.version = RUNTIME_PROTOCOL_VERSION;
        }
        if self.idempotency_key.trim().is_empty() {
            self.idempotency_key = new_runtime_id();
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeHostSurface {
    #[default]
    Desktop,
    Cli,
    Ide,
    Mcp,
    Acp,
    Gateway,
    Test,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSourceScope {
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub collection_id: Option<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSkillContext {
    #[serde(default)]
    pub available_skill_ids: Vec<String>,
    #[serde(default)]
    pub loaded_skill_ids: Vec<String>,
    #[serde(default)]
    pub trust_state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePackageContext {
    #[serde(default)]
    pub enabled_package_ids: Vec<String>,
    #[serde(default)]
    pub disabled_package_ids: Vec<String>,
}

impl RuntimePackageContext {
    pub fn from_package_host_snapshot(snapshot: &crate::package_host::PackageHostSnapshot) -> Self {
        let mut enabled_package_ids = Vec::new();
        let mut disabled_package_ids = Vec::new();
        for record in &snapshot.records {
            if record.is_runtime_visible() {
                enabled_package_ids.push(record.id.clone());
            } else {
                disabled_package_ids.push(record.id.clone());
            }
        }
        enabled_package_ids.sort();
        disabled_package_ids.sort();
        Self {
            enabled_package_ids,
            disabled_package_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionConfig {
    #[serde(default = "runtime_protocol_version")]
    pub version: u16,
    #[serde(default = "new_runtime_id")]
    pub session_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub task_run_id: Option<String>,
    #[serde(default)]
    pub host_surface: RuntimeHostSurface,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_enabled: Option<bool>,
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub source_scope: RuntimeSourceScope,
    #[serde(default)]
    pub approval_mode: ToolApprovalMode,
    #[serde(default)]
    pub shell_access_mode: ShellAccessMode,
    #[serde(default)]
    pub execution_mode: AgentExecutionMode,
    #[serde(default)]
    pub collaboration_mode: AgentCollaborationMode,
    #[serde(default)]
    pub moa_preset: MoaPresetId,
    #[serde(default)]
    pub orchestration_profile: OrchestrationProfile,
    #[serde(default)]
    pub custom_orchestration: Option<CustomOrchestrationOptions>,
    #[serde(default = "default_trace_enabled")]
    pub trace_enabled: bool,
    #[serde(default)]
    pub skill_context: RuntimeSkillContext,
    #[serde(default)]
    pub package_context: RuntimePackageContext,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Default for AgentSessionConfig {
    fn default() -> Self {
        Self {
            version: RUNTIME_PROTOCOL_VERSION,
            session_id: new_runtime_id(),
            conversation_id: None,
            task_run_id: None,
            host_surface: RuntimeHostSurface::Desktop,
            provider: None,
            model: None,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            source_scope: RuntimeSourceScope::default(),
            approval_mode: ToolApprovalMode::default(),
            shell_access_mode: ShellAccessMode::Restricted,
            execution_mode: AgentExecutionMode::Normal,
            collaboration_mode: AgentCollaborationMode::Direct,
            moa_preset: MoaPresetId::FastReview,
            orchestration_profile: OrchestrationProfile::Balanced,
            custom_orchestration: None,
            trace_enabled: true,
            skill_context: RuntimeSkillContext::default(),
            package_context: RuntimePackageContext::default(),
            metadata: serde_json::json!({}),
        }
    }
}

impl AgentSessionConfig {
    pub fn from_versioned_json(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let mut config: Self = serde_json::from_value(value)?;
        config.apply_protocol_defaults();
        Ok(config)
    }

    pub fn apply_protocol_defaults(&mut self) {
        if self.version == 0 {
            self.version = RUNTIME_PROTOCOL_VERSION;
        }
        if self.session_id.trim().is_empty() {
            self.session_id = new_runtime_id();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnAttachment {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnInput {
    pub user_text: String,
    #[serde(default)]
    pub attachments: Vec<AgentTurnAttachment>,
    #[serde(default)]
    pub selected_source_ids: Vec<String>,
    #[serde(default)]
    pub selected_collection_id: Option<String>,
    #[serde(default)]
    pub mode: AgentExecutionMode,
    #[serde(default)]
    pub host_metadata: serde_json::Value,
}

impl AgentTurnInput {
    pub fn text(user_text: impl Into<String>) -> Self {
        Self {
            user_text: user_text.into(),
            attachments: Vec::new(),
            selected_source_ids: Vec::new(),
            selected_collection_id: None,
            mode: AgentExecutionMode::Normal,
            host_metadata: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTerminalStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Paused,
}

impl RuntimeTerminalStatus {
    pub fn from_run_event(event: &AgentRunEvent) -> Option<Self> {
        if !event.is_terminal() {
            return None;
        }
        match event.status.as_deref() {
            Some("cancelled") => Some(Self::Cancelled),
            Some("timed_out") => Some(Self::TimedOut),
            Some("paused") => Some(Self::Paused),
            Some("completed") => Some(Self::Completed),
            _ => Some(Self::Failed),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Paused => "paused",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentTurnState {
    Starting,
    Running,
    WaitingApproval,
    AwaitingUserInput,
    Terminal(RuntimeTerminalStatus),
}

/// Stable stage names shared by backend launch telemetry and frontend paint
/// instrumentation. Values include their unit so exported metrics remain
/// self-describing across protocol boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnLaunchStage {
    LaunchAckMs,
    HistoryLoadMs,
    ContextBuildMs,
    SkillSelectMs,
    McpSyncMs,
    ToolRegistryMs,
    AttachmentPrepareMs,
    RequestBuildMs,
    ProviderConnectMs,
    FirstSseByteMs,
    FirstVisibleTokenMs,
    FrontendFirstPaintMs,
}

impl TurnLaunchStage {
    pub const ALL: [Self; 12] = [
        Self::LaunchAckMs,
        Self::HistoryLoadMs,
        Self::ContextBuildMs,
        Self::SkillSelectMs,
        Self::McpSyncMs,
        Self::ToolRegistryMs,
        Self::AttachmentPrepareMs,
        Self::RequestBuildMs,
        Self::ProviderConnectMs,
        Self::FirstSseByteMs,
        Self::FirstVisibleTokenMs,
        Self::FrontendFirstPaintMs,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LaunchAckMs => "launch_ack_ms",
            Self::HistoryLoadMs => "history_load_ms",
            Self::ContextBuildMs => "context_build_ms",
            Self::SkillSelectMs => "skill_select_ms",
            Self::McpSyncMs => "mcp_sync_ms",
            Self::ToolRegistryMs => "tool_registry_ms",
            Self::AttachmentPrepareMs => "attachment_prepare_ms",
            Self::RequestBuildMs => "request_build_ms",
            Self::ProviderConnectMs => "provider_connect_ms",
            Self::FirstSseByteMs => "first_sse_byte_ms",
            Self::FirstVisibleTokenMs => "first_visible_token_ms",
            Self::FrontendFirstPaintMs => "frontend_first_paint_ms",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnHandle {
    pub session_id: String,
    pub run_id: String,
    pub turn_id: String,
    pub state: AgentTurnState,
}

impl AgentTurnHandle {
    pub fn running(
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            run_id: run_id.into(),
            turn_id: turn_id.into(),
            state: AgentTurnState::Running,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum RuntimeOperation {
    ConfigureSession {
        config: AgentSessionConfig,
    },
    StartTurn {
        input: AgentTurnInput,
    },
    SteerTurn {
        turn_id: String,
        text: String,
    },
    InterruptTurn {
        turn_id: String,
        reason: String,
    },
    ResolveApproval {
        request_id: String,
        decision: ApprovalDecision,
    },
    ReadEvents {
        run_id: String,
    },
    ResumeSession {
        session_id: String,
    },
    CloseSession {
        session_id: String,
    },
    StartContextCompaction {
        request: StartContextCompactionRequest,
    },
    CancelOperation {
        operation_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnResult {
    pub handle: AgentTurnHandle,
    pub final_message: Option<String>,
    pub artifacts: serde_json::Value,
    pub usage: serde_json::Value,
    pub trace_id: Option<String>,
    pub status: RuntimeTerminalStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionDependencyContract {
    pub provider_adapter: bool,
    pub tool_registry: bool,
    pub package_host: bool,
    pub skill_activation: bool,
    pub approval_adapter: bool,
    pub persistence_adapter: bool,
    pub execution_environment: bool,
}

impl AgentSessionDependencyContract {
    pub fn complete_runtime() -> Self {
        Self {
            provider_adapter: true,
            tool_registry: true,
            package_host: true,
            skill_activation: true,
            approval_adapter: true,
            persistence_adapter: true,
            execution_environment: true,
        }
    }
}

#[async_trait]
pub trait AgentSession: Send {
    fn config(&self) -> &AgentSessionConfig;

    async fn configure(&mut self, config: AgentSessionConfig) -> Result<(), CoreError>;

    async fn start_turn(&mut self, input: AgentTurnInput) -> Result<AgentTurnHandle, CoreError>;

    async fn steer_turn(&mut self, turn_id: &str, text: String) -> Result<(), CoreError>;

    async fn interrupt_turn(&mut self, turn_id: &str, reason: String) -> Result<(), CoreError>;

    async fn resolve_approval(
        &mut self,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), CoreError>;

    async fn read_events(&self, run_id: &str) -> Result<Vec<AgentRunEvent>, CoreError>;

    async fn close(&mut self) -> Result<(), CoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTurnContractReport {
    pub run_id: String,
    pub turn_id: String,
    pub event_count: usize,
    pub terminal_status: RuntimeTerminalStatus,
    pub approval_denied: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuntimeProtocolError {
    #[error("turn emitted no events")]
    Empty,
    #[error("event {event_seq} has unsupported run event version {version}")]
    UnsupportedRunEventVersion { event_seq: u64, version: u16 },
    #[error("event {event_seq} has an empty run id")]
    MissingRunId { event_seq: u64 },
    #[error("event {event_seq} has an empty turn id")]
    MissingTurnId { event_seq: u64 },
    #[error("event sequence is not monotonic: expected {expected}, got {actual}")]
    EventSequenceGap { expected: u64, actual: u64 },
    #[error("events from multiple runs were mixed")]
    MixedRunIds,
    #[error("events from multiple turns were mixed")]
    MixedTurnIds,
    #[error("turn must emit exactly one terminal event, got {count}")]
    TerminalEventCount { count: usize },
    #[error("terminal event must be the final event in a turn")]
    TerminalNotLast,
}

pub fn validate_runtime_turn_events(
    events: &[AgentRunEvent],
) -> Result<RuntimeTurnContractReport, RuntimeProtocolError> {
    let first = events.first().ok_or(RuntimeProtocolError::Empty)?;
    let run_id = first.run_id.clone();
    let turn_id = first.turn_id.clone();

    if run_id.trim().is_empty() {
        return Err(RuntimeProtocolError::MissingRunId {
            event_seq: first.event_seq,
        });
    }
    if turn_id.trim().is_empty() {
        return Err(RuntimeProtocolError::MissingTurnId {
            event_seq: first.event_seq,
        });
    }

    let mut terminal_indexes = Vec::new();
    let mut approval_denied = false;

    for (expected_seq, (index, event)) in (first.event_seq..).zip(events.iter().enumerate()) {
        if event.version != AGENT_RUN_EVENT_VERSION {
            return Err(RuntimeProtocolError::UnsupportedRunEventVersion {
                event_seq: event.event_seq,
                version: event.version,
            });
        }
        if event.run_id != run_id {
            return Err(RuntimeProtocolError::MixedRunIds);
        }
        if event.turn_id != turn_id {
            return Err(RuntimeProtocolError::MixedTurnIds);
        }
        if event.event_seq != expected_seq {
            return Err(RuntimeProtocolError::EventSequenceGap {
                expected: expected_seq,
                actual: event.event_seq,
            });
        }
        if event.is_terminal() {
            terminal_indexes.push(index);
        }
        if event.kind.as_str() == "approvalResolved"
            && matches!(event.status.as_deref(), Some("denied"))
        {
            approval_denied = true;
        }
    }

    if terminal_indexes.len() != 1 {
        return Err(RuntimeProtocolError::TerminalEventCount {
            count: terminal_indexes.len(),
        });
    }
    let terminal_index = terminal_indexes[0];
    if terminal_index + 1 != events.len() {
        return Err(RuntimeProtocolError::TerminalNotLast);
    }

    let terminal_event = &events[terminal_index];
    let terminal_status = RuntimeTerminalStatus::from_run_event(terminal_event)
        .unwrap_or(RuntimeTerminalStatus::Failed);

    Ok(RuntimeTurnContractReport {
        run_id,
        turn_id,
        event_count: events.len(),
        terminal_status,
        approval_denied,
    })
}

fn runtime_protocol_version() -> u16 {
    RUNTIME_PROTOCOL_VERSION
}

fn default_trace_enabled() -> bool {
    true
}

fn new_runtime_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::agent_run::{
        AgentRunDisplayKind, AgentRunEventImportance, AgentRunEventKind, AgentRunEventPersistence,
        AgentRunEventVisibility, AgentRunPhase,
    };

    #[test]
    fn turn_launch_stages_expose_a_stable_cross_layer_metric_contract() {
        assert_eq!(
            TurnLaunchStage::ALL.map(TurnLaunchStage::as_str),
            [
                "launch_ack_ms",
                "history_load_ms",
                "context_build_ms",
                "skill_select_ms",
                "mcp_sync_ms",
                "tool_registry_ms",
                "attachment_prepare_ms",
                "request_build_ms",
                "provider_connect_ms",
                "first_sse_byte_ms",
                "first_visible_token_ms",
                "frontend_first_paint_ms",
            ]
        );
    }

    #[test]
    fn run_event_outbox_accepts_one_terminal_and_rejects_later_events() {
        let (outbox, mut receiver) = AgentRunEventOutbox::channel();
        let event =
            AgentRunEvent::terminal_status("run-1", Some("turn-1"), 0, "done", "completed", None);
        outbox
            .submit(event)
            .expect("terminal event should be accepted");
        assert!(outbox
            .submit(AgentRunEvent::terminal_status(
                "run-1",
                Some("turn-1"),
                0,
                "late",
                "completed",
                None
            ))
            .is_err());
        assert!(receiver.try_recv().is_ok());
    }

    #[test]
    fn run_event_outbox_clones_share_one_ordered_producer_queue() {
        let (outbox, mut receiver) = AgentRunEventOutbox::channel();
        let continuation = outbox.clone();
        outbox
            .submit(AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                0,
                AgentRunPhase::AwaitingUserInput,
                "Waiting for your response",
                Some("awaiting_user_input"),
                None,
            ))
            .unwrap();
        continuation
            .submit(AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                0,
                AgentRunPhase::Responding,
                "Interaction response accepted",
                Some("running"),
                None,
            ))
            .unwrap();

        assert_eq!(
            receiver.try_recv().unwrap().label,
            "Waiting for your response"
        );
        assert_eq!(
            receiver.try_recv().unwrap().label,
            "Interaction response accepted"
        );
    }

    #[test]
    fn run_event_outbox_failure_closes_submission_and_cancels_turn_children() {
        let (outbox, _receiver) = AgentRunEventOutbox::channel();
        let suspended_turn = outbox.turn_cancellation_token();
        suspended_turn.cancel();
        let existing_turn = outbox.turn_cancellation_token();
        let failure_handle = outbox.failure_handle();

        assert!(!existing_turn.is_cancelled());
        failure_handle.fail_closed();

        assert!(outbox.is_closed_for_submission());
        assert!(existing_turn.is_cancelled());
        assert!(outbox.turn_cancellation_token().is_cancelled());
        assert!(outbox
            .submit(AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                0,
                AgentRunPhase::Responding,
                "late",
                Some("running"),
                None,
            ))
            .is_err());
    }

    #[test]
    fn run_event_outbox_capacity_fails_closed_before_ordered_events_can_be_dropped() {
        let (outbox, _receiver) = AgentRunEventOutbox::channel();
        let cancellation = outbox.turn_cancellation_token();
        for index in 0..AGENT_RUN_EVENT_OUTBOX_CAPACITY {
            outbox
                .submit(AgentRunEvent::status_update(
                    "run-1",
                    Some("turn-1"),
                    0,
                    AgentRunPhase::Responding,
                    &format!("queued-{index}"),
                    Some("running"),
                    None,
                ))
                .expect("bounded queue still has capacity");
        }

        let error = outbox
            .submit(AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                0,
                AgentRunPhase::Responding,
                "must not be dropped",
                Some("running"),
                None,
            ))
            .expect_err("overflow must fail closed");

        assert!(error.to_string().contains("capacity was exhausted"));
        assert!(cancellation.is_cancelled());
        assert!(outbox.is_closed_for_submission());
    }

    #[tokio::test]
    async fn session_manager_replaces_turns_and_routes_steering() {
        let manager = AgentSessionManager::new();
        let first_cancel = CancellationToken::new();
        let first_cancel_observer = first_cancel.clone();
        let (first_tx, _first_rx) = tokio::sync::mpsc::unbounded_channel();
        manager
            .register(ActiveAgentTurn {
                handle: AgentTurnHandle::running("session-1", "run-1", "turn-1"),
                cancel_token: first_cancel,
                task: tokio::spawn(std::future::pending::<()>()),
                steering_tx: first_tx,
                event_outbox: Arc::new(AgentRunEventOutbox::channel().0),
                orchestrator_run_id: None,
                frontend_paint_recorded: AtomicBool::new(false),
            })
            .await;

        let second_cancel = CancellationToken::new();
        let (second_tx, mut second_rx) = tokio::sync::mpsc::unbounded_channel();
        manager
            .register(ActiveAgentTurn {
                handle: AgentTurnHandle::running("session-1", "run-2", "turn-2"),
                cancel_token: second_cancel,
                task: tokio::spawn(std::future::pending::<()>()),
                steering_tx: second_tx,
                event_outbox: Arc::new(AgentRunEventOutbox::channel().0),
                orchestrator_run_id: None,
                frontend_paint_recorded: AtomicBool::new(false),
            })
            .await;

        assert!(first_cancel_observer.is_cancelled());
        assert!(manager
            .take_for_run("session-1", "run-other")
            .await
            .is_err());
        manager
            .steer("session-1", "keep going")
            .await
            .expect("active turn should accept steering");
        assert_eq!(
            second_rx.recv().await.expect("steering message").content,
            "keep going"
        );
        assert!(
            !manager
                .claim_frontend_paint_metric("session-1", "run-1", "turn-1")
                .await
        );
        assert!(
            manager
                .claim_frontend_paint_metric("session-1", "run-2", "turn-2")
                .await
        );
        assert!(
            !manager
                .claim_frontend_paint_metric("session-1", "run-2", "turn-2")
                .await
        );

        let active = manager.take("session-1").await.expect("active turn");
        active.cancel();
        active.abort();
        assert!(!manager.contains("session-1").await);
    }

    #[tokio::test]
    async fn session_manager_retains_open_outbox_after_suspended_task_finishes() {
        let manager = AgentSessionManager::new();
        let (outbox, _receiver) = AgentRunEventOutbox::channel();
        let outbox = Arc::new(outbox);
        let (steering_tx, _steering_rx) = tokio::sync::mpsc::unbounded_channel();
        manager
            .register(ActiveAgentTurn {
                handle: AgentTurnHandle::running("session-1", "run-1", "turn-1"),
                cancel_token: CancellationToken::new(),
                task: tokio::spawn(async {}),
                steering_tx,
                event_outbox: Arc::clone(&outbox),
                orchestrator_run_id: None,
                frontend_paint_recorded: AtomicBool::new(false),
            })
            .await;
        tokio::task::yield_now().await;

        assert!(!manager.contains("session-1").await);
        assert!(manager.try_is_empty().unwrap());
        let retained = manager
            .take("session-1")
            .await
            .expect("finished suspended turn should retain its open outbox");
        assert!(Arc::ptr_eq(&retained.event_outbox, &outbox));
    }

    #[test]
    fn session_config_defaults_are_runtime_protocol_v1() {
        let config = AgentSessionConfig::default();

        assert_eq!(config.version, RUNTIME_PROTOCOL_VERSION);
        assert!(!config.session_id.trim().is_empty());
        assert_eq!(config.host_surface, RuntimeHostSurface::Desktop);
        assert_eq!(config.shell_access_mode, ShellAccessMode::Restricted);
        assert_eq!(config.execution_mode, AgentExecutionMode::Normal);
        assert!(config.trace_enabled);
    }

    #[test]
    fn session_config_migrates_missing_or_zero_version() {
        let raw = serde_json::json!({
            "version": 0,
            "sessionId": "",
            "hostSurface": "cli",
            "model": "gpt-test"
        });

        let config = AgentSessionConfig::from_versioned_json(raw).unwrap();

        assert_eq!(config.version, RUNTIME_PROTOCOL_VERSION);
        assert!(!config.session_id.trim().is_empty());
        assert_eq!(config.host_surface, RuntimeHostSurface::Cli);
        assert_eq!(config.model.as_deref(), Some("gpt-test"));
    }

    #[test]
    fn runtime_package_context_projects_package_host_visibility() {
        let snapshot = crate::package_host::PackageHostSnapshot::new(vec![
            crate::package_host::PackageHostRecord {
                id: "pkg-enabled".to_string(),
                version: None,
                state: crate::package_host::PackageLifecycleState::Enabled,
                health: crate::package_host::PackageHealthState::Healthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: Vec::new(),
            },
            crate::package_host::PackageHostRecord {
                id: "pkg-disabled".to_string(),
                version: None,
                state: crate::package_host::PackageLifecycleState::Disabled,
                health: crate::package_host::PackageHealthState::Healthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: Vec::new(),
            },
            crate::package_host::PackageHostRecord {
                id: "pkg-unhealthy".to_string(),
                version: None,
                state: crate::package_host::PackageLifecycleState::Enabled,
                health: crate::package_host::PackageHealthState::Unhealthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: Vec::new(),
            },
        ]);

        let context = RuntimePackageContext::from_package_host_snapshot(&snapshot);

        assert_eq!(context.enabled_package_ids, vec!["pkg-enabled".to_string()]);
        assert_eq!(
            context.disabled_package_ids,
            vec!["pkg-disabled".to_string(), "pkg-unhealthy".to_string()]
        );
    }

    #[test]
    fn runtime_turn_events_accept_one_terminal_event() {
        let events = vec![
            AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                1,
                AgentRunPhase::Routing,
                "Route selected: Direct",
                Some("running"),
                None,
            ),
            AgentRunEvent::from_agent_event(&AgentEvent::Done {
                message: crate::llm::Message::text(crate::llm::Role::Assistant, "done"),
                usage_total: crate::llm::Usage::default(),
                last_prompt_tokens: 0,
                context_breakdown: None,
                cached: false,
                finish_reason: Some("stop".to_string()),
            })
            .with_context(Some("run-1"), Some("turn-1"), Some(2)),
        ];

        let report = validate_runtime_turn_events(&events).unwrap();

        assert_eq!(report.run_id, "run-1");
        assert_eq!(report.turn_id, "turn-1");
        assert_eq!(report.terminal_status, RuntimeTerminalStatus::Completed);
        assert_eq!(report.event_count, 2);
    }

    #[test]
    fn runtime_turn_events_reject_sequence_gap() {
        let events = vec![
            AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                1,
                AgentRunPhase::Routing,
                "Route selected: Direct",
                Some("running"),
                None,
            ),
            AgentRunEvent::terminal_error("run-1", Some("turn-1"), 3, "failed", "failed", None),
        ];

        assert_eq!(
            validate_runtime_turn_events(&events).unwrap_err(),
            RuntimeProtocolError::EventSequenceGap {
                expected: 2,
                actual: 3
            }
        );
    }

    #[test]
    fn runtime_turn_events_reject_multiple_terminal_events() {
        let events = vec![
            AgentRunEvent::terminal_error("run-1", Some("turn-1"), 1, "failed", "failed", None),
            AgentRunEvent {
                version: AGENT_RUN_EVENT_VERSION,
                run_id: "run-1".to_string(),
                turn_id: "turn-1".to_string(),
                event_seq: 2,
                kind: AgentRunEventKind::Done,
                phase: AgentRunPhase::Done,
                visibility: AgentRunEventVisibility::User,
                persistence: AgentRunEventPersistence::Durable,
                display_kind: AgentRunDisplayKind::Completion,
                importance: AgentRunEventImportance::High,
                label: "done".to_string(),
                status: Some("completed".to_string()),
                payload: serde_json::json!({ "message": "done" }),
                created_at: None,
            },
        ];

        assert_eq!(
            validate_runtime_turn_events(&events).unwrap_err(),
            RuntimeProtocolError::TerminalEventCount { count: 2 }
        );
    }
}
