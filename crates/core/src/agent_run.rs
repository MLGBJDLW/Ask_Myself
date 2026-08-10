//! Stable run protocol for agent execution.
//!
//! `AgentEvent` is optimized for the live executor loop. `AgentRunEvent` is the
//! durable Interface for replay, task timelines, and future clients.

use serde::{Deserialize, Serialize};

use crate::agent::{AgentEvent, StreamBlockChannel, ToolRunItem, ToolRunStatus};

pub const AGENT_RUN_EVENT_VERSION: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunPhase {
    Routing,
    Planning,
    Responding,
    Tooling,
    Approval,
    AwaitingUserInput,
    Compacting,
    Accounting,
    Done,
}

impl AgentRunPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::Planning => "planning",
            Self::Responding => "responding",
            Self::Tooling => "tooling",
            Self::Approval => "approval",
            Self::AwaitingUserInput => "awaiting_user_input",
            Self::Compacting => "compacting",
            Self::Accounting => "accounting",
            Self::Done => "done",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "routing" => Some(Self::Routing),
            "planning" => Some(Self::Planning),
            "responding" => Some(Self::Responding),
            "tooling" => Some(Self::Tooling),
            "approval" => Some(Self::Approval),
            "awaiting_user_input" => Some(Self::AwaitingUserInput),
            "compacting" => Some(Self::Compacting),
            "accounting" => Some(Self::Accounting),
            "done" => Some(Self::Done),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunEventKind {
    OutputDelta,
    StreamReset,
    Thinking,
    Status,
    PlanUpdated,
    ToolPreparing,
    ToolStarted,
    ToolProgress,
    ToolCompleted,
    ApprovalRequested,
    ApprovalResolved,
    RecoveryAttempt,
    UsageUpdated,
    AutoCompacted,
    Done,
    Error,
}

impl AgentRunEventKind {
    pub const ALL: [Self; 16] = [
        Self::OutputDelta,
        Self::StreamReset,
        Self::Thinking,
        Self::Status,
        Self::PlanUpdated,
        Self::ToolPreparing,
        Self::ToolStarted,
        Self::ToolProgress,
        Self::ToolCompleted,
        Self::ApprovalRequested,
        Self::ApprovalResolved,
        Self::RecoveryAttempt,
        Self::UsageUpdated,
        Self::AutoCompacted,
        Self::Done,
        Self::Error,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutputDelta => "outputDelta",
            Self::StreamReset => "streamReset",
            Self::Thinking => "thinking",
            Self::Status => "status",
            Self::PlanUpdated => "planUpdated",
            Self::ToolPreparing => "toolPreparing",
            Self::ToolStarted => "toolStarted",
            Self::ToolProgress => "toolProgress",
            Self::ToolCompleted => "toolCompleted",
            Self::ApprovalRequested => "approvalRequested",
            Self::ApprovalResolved => "approvalResolved",
            Self::RecoveryAttempt => "recoveryAttempt",
            Self::UsageUpdated => "usageUpdated",
            Self::AutoCompacted => "autoCompacted",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "outputDelta" => Some(Self::OutputDelta),
            "streamReset" => Some(Self::StreamReset),
            "thinking" => Some(Self::Thinking),
            "status" => Some(Self::Status),
            "planUpdated" => Some(Self::PlanUpdated),
            "toolPreparing" => Some(Self::ToolPreparing),
            "toolStarted" => Some(Self::ToolStarted),
            "toolProgress" => Some(Self::ToolProgress),
            "toolCompleted" => Some(Self::ToolCompleted),
            "approvalRequested" => Some(Self::ApprovalRequested),
            "approvalResolved" => Some(Self::ApprovalResolved),
            "recoveryAttempt" => Some(Self::RecoveryAttempt),
            "usageUpdated" => Some(Self::UsageUpdated),
            "autoCompacted" => Some(Self::AutoCompacted),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunEventVisibility {
    #[default]
    User,
    Developer,
    Internal,
}

impl AgentRunEventVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Developer => "developer",
            Self::Internal => "internal",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "developer" => Some(Self::Developer),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunEventPersistence {
    #[default]
    Durable,
    Ephemeral,
}

impl AgentRunEventPersistence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Ephemeral => "ephemeral",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "durable" => Some(Self::Durable),
            "ephemeral" => Some(Self::Ephemeral),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunDisplayKind {
    Output,
    Reasoning,
    #[default]
    Status,
    Plan,
    Tool,
    Approval,
    Recovery,
    Steering,
    Usage,
    Compaction,
    Completion,
    Error,
}

impl AgentRunDisplayKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Reasoning => "reasoning",
            Self::Status => "status",
            Self::Plan => "plan",
            Self::Tool => "tool",
            Self::Approval => "approval",
            Self::Recovery => "recovery",
            Self::Steering => "steering",
            Self::Usage => "usage",
            Self::Compaction => "compaction",
            Self::Completion => "completion",
            Self::Error => "error",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "output" => Some(Self::Output),
            "reasoning" => Some(Self::Reasoning),
            "status" => Some(Self::Status),
            "plan" => Some(Self::Plan),
            "tool" => Some(Self::Tool),
            "approval" => Some(Self::Approval),
            "recovery" => Some(Self::Recovery),
            "steering" => Some(Self::Steering),
            "usage" => Some(Self::Usage),
            "compaction" => Some(Self::Compaction),
            "completion" => Some(Self::Completion),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunEventImportance {
    Low,
    #[default]
    Normal,
    High,
}

impl AgentRunEventImportance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

fn event_presentation(
    kind: AgentRunEventKind,
) -> (
    AgentRunEventVisibility,
    AgentRunDisplayKind,
    AgentRunEventImportance,
) {
    match kind {
        AgentRunEventKind::OutputDelta => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Output,
            AgentRunEventImportance::Normal,
        ),
        AgentRunEventKind::Thinking => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Reasoning,
            AgentRunEventImportance::Normal,
        ),
        AgentRunEventKind::PlanUpdated => (
            AgentRunEventVisibility::Developer,
            AgentRunDisplayKind::Plan,
            AgentRunEventImportance::Normal,
        ),
        AgentRunEventKind::ToolPreparing
        | AgentRunEventKind::ToolStarted
        | AgentRunEventKind::ToolProgress
        | AgentRunEventKind::ToolCompleted => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Tool,
            AgentRunEventImportance::Normal,
        ),
        AgentRunEventKind::ApprovalRequested | AgentRunEventKind::ApprovalResolved => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Approval,
            AgentRunEventImportance::High,
        ),
        AgentRunEventKind::StreamReset | AgentRunEventKind::RecoveryAttempt => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Recovery,
            AgentRunEventImportance::High,
        ),
        AgentRunEventKind::UsageUpdated => (
            AgentRunEventVisibility::Developer,
            AgentRunDisplayKind::Usage,
            AgentRunEventImportance::Low,
        ),
        AgentRunEventKind::AutoCompacted => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Compaction,
            AgentRunEventImportance::Normal,
        ),
        AgentRunEventKind::Done => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Completion,
            AgentRunEventImportance::High,
        ),
        AgentRunEventKind::Error => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Error,
            AgentRunEventImportance::High,
        ),
        AgentRunEventKind::Status => (
            AgentRunEventVisibility::User,
            AgentRunDisplayKind::Status,
            AgentRunEventImportance::Normal,
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunEventKindContract {
    pub kind: AgentRunEventKind,
    pub required_payload_paths: &'static [&'static str],
    pub alternative_payload_paths: &'static [&'static [&'static str]],
}

impl AgentRunEventKindContract {
    pub fn for_kind(kind: AgentRunEventKind) -> Self {
        match kind {
            AgentRunEventKind::OutputDelta => Self {
                kind,
                required_payload_paths: &["delta"],
                alternative_payload_paths: &[&["blockId", "channel", "offset", "delta"]],
            },
            AgentRunEventKind::StreamReset => Self {
                kind,
                required_payload_paths: &["reason"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::Thinking => Self {
                kind,
                required_payload_paths: &["content"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::Status => Self {
                kind,
                required_payload_paths: &["content"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::PlanUpdated => Self {
                kind,
                required_payload_paths: &["plan"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::ToolPreparing => Self {
                kind,
                required_payload_paths: &["run.callId", "run.toolName", "run.status"],
                // Read compatibility for already-persisted protocol-v2 rows.
                // New producers are asserted separately to emit payload.run.
                alternative_payload_paths: &[&["toolName"]],
            },
            AgentRunEventKind::ToolStarted => Self {
                kind,
                required_payload_paths: &["run.callId", "run.toolName", "run.status"],
                alternative_payload_paths: &[&["toolName"]],
            },
            AgentRunEventKind::ToolProgress => Self {
                kind,
                required_payload_paths: &["run.callId", "run.toolName", "run.status"],
                alternative_payload_paths: &[&["note"]],
            },
            AgentRunEventKind::ToolCompleted => Self {
                kind,
                required_payload_paths: &["run.callId", "run.toolName", "run.status"],
                alternative_payload_paths: &[&["toolName"]],
            },
            AgentRunEventKind::ApprovalRequested => Self {
                kind,
                required_payload_paths: &["request"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::ApprovalResolved => Self {
                kind,
                required_payload_paths: &["requestId", "decision"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::RecoveryAttempt => Self {
                kind,
                required_payload_paths: &["reason"],
                alternative_payload_paths: &[&[
                    "state.state",
                    "state.providerId",
                    "state.modelId",
                    "state.attempt",
                    "state.maxAttempts",
                ]],
            },
            AgentRunEventKind::UsageUpdated => Self {
                kind,
                required_payload_paths: &["usageTotal", "lastPromptTokens"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::AutoCompacted => Self {
                kind,
                required_payload_paths: &["evictedCount"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::Done => Self {
                kind,
                required_payload_paths: &["message", "usageTotal"],
                alternative_payload_paths: &[],
            },
            AgentRunEventKind::Error => Self {
                kind,
                required_payload_paths: &["message"],
                alternative_payload_paths: &[],
            },
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgentRunEventContractError {
    #[error("unsupported event version {version}; expected {expected}")]
    UnsupportedVersion { version: u16, expected: u16 },
    #[error("durable event is missing run_id")]
    MissingRunId,
    #[error("durable event is missing turn_id")]
    MissingTurnId,
    #[error("durable event_seq must be greater than zero")]
    MissingEventSequence,
    #[error("{kind} event is missing payload field `{path}`")]
    MissingPayloadField {
        kind: &'static str,
        path: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunEvent {
    pub version: u16,
    pub run_id: String,
    pub turn_id: String,
    pub event_seq: u64,
    pub kind: AgentRunEventKind,
    pub phase: AgentRunPhase,
    #[serde(default)]
    pub visibility: AgentRunEventVisibility,
    #[serde(default)]
    pub persistence: AgentRunEventPersistence,
    #[serde(default)]
    pub display_kind: AgentRunDisplayKind,
    #[serde(default)]
    pub importance: AgentRunEventImportance,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl AgentRunEvent {
    pub fn from_agent_event(event: &AgentEvent) -> Self {
        let (kind, phase, label, status) = match event {
            AgentEvent::TextDelta { .. } | AgentEvent::StreamBlockDelta { .. } => (
                AgentRunEventKind::OutputDelta,
                AgentRunPhase::Responding,
                "Assistant response".to_string(),
                Some("streaming".to_string()),
            ),
            AgentEvent::StreamReset { reason } => recovery_metadata(reason),
            AgentEvent::ToolCallPreparing { tool_name, .. }
            | AgentEvent::ToolCallArgsDelta { tool_name, .. } => (
                AgentRunEventKind::ToolPreparing,
                AgentRunPhase::Tooling,
                tool_name.clone(),
                Some(ToolRunStatus::Preparing.as_str().to_string()),
            ),
            AgentEvent::ToolCallStart { tool_name, .. } => (
                AgentRunEventKind::ToolStarted,
                AgentRunPhase::Tooling,
                tool_name.clone(),
                Some(ToolRunStatus::Running.as_str().to_string()),
            ),
            AgentEvent::ToolCallProgress { tool_name, .. } => (
                AgentRunEventKind::ToolProgress,
                AgentRunPhase::Tooling,
                tool_name.clone(),
                Some(ToolRunStatus::Running.as_str().to_string()),
            ),
            AgentEvent::ToolCallResult {
                tool_name,
                is_error,
                ..
            } => (
                AgentRunEventKind::ToolCompleted,
                AgentRunPhase::Tooling,
                tool_name.clone(),
                Some(
                    if *is_error {
                        ToolRunStatus::Failed
                    } else {
                        ToolRunStatus::Completed
                    }
                    .as_str()
                    .to_string(),
                ),
            ),
            AgentEvent::ToolRunStarted { run } => (
                AgentRunEventKind::ToolStarted,
                AgentRunPhase::Tooling,
                run.tool_name.clone(),
                Some(run.status.as_str().to_string()),
            ),
            AgentEvent::ToolRunUpdated { run } => (
                AgentRunEventKind::ToolProgress,
                AgentRunPhase::Tooling,
                run.progress_note
                    .clone()
                    .unwrap_or_else(|| run.tool_name.clone()),
                Some(run.status.as_str().to_string()),
            ),
            AgentEvent::ToolRunCompleted { run } => (
                AgentRunEventKind::ToolCompleted,
                AgentRunPhase::Tooling,
                run.tool_name.clone(),
                Some(run.status.as_str().to_string()),
            ),
            AgentEvent::Thinking { .. } => (
                AgentRunEventKind::Thinking,
                AgentRunPhase::Responding,
                "Reasoning update".to_string(),
                Some("running".to_string()),
            ),
            AgentEvent::Status { content, tone } => (
                AgentRunEventKind::Status,
                AgentRunPhase::Responding,
                content.clone(),
                tone.clone().or_else(|| Some("running".to_string())),
            ),
            AgentEvent::ControllerStatus {
                code,
                content,
                tone,
            } => (
                AgentRunEventKind::Status,
                controller_status_phase(code),
                content.clone(),
                tone.clone().or_else(|| Some("running".to_string())),
            ),
            AgentEvent::ConnectionState { state } => {
                let (label, status) = match state.state {
                    crate::agent::ConnectionStateKind::Degraded => {
                        ("Provider connection degraded", "degraded")
                    }
                    crate::agent::ConnectionStateKind::Reconnecting => {
                        ("Reconnecting to provider", "reconnecting")
                    }
                    crate::agent::ConnectionStateKind::Recovered => {
                        ("Provider connection recovered", "recovered")
                    }
                    crate::agent::ConnectionStateKind::Offline => {
                        ("Provider is offline", "offline")
                    }
                    crate::agent::ConnectionStateKind::Failed => {
                        ("Provider connection failed", "failed")
                    }
                };
                (
                    AgentRunEventKind::RecoveryAttempt,
                    AgentRunPhase::Responding,
                    label.to_string(),
                    Some(status.to_string()),
                )
            }
            AgentEvent::Steering { content } => (
                AgentRunEventKind::Status,
                AgentRunPhase::Responding,
                content.clone(),
                Some("accepted".to_string()),
            ),
            AgentEvent::PlanUpdated { summary, phase, .. } => (
                AgentRunEventKind::PlanUpdated,
                phase
                    .as_deref()
                    .map(plan_phase)
                    .unwrap_or(AgentRunPhase::Planning),
                summary
                    .clone()
                    .unwrap_or_else(|| "Execution plan updated".to_string()),
                Some("running".to_string()),
            ),
            AgentEvent::Done { finish_reason, .. } => {
                let status = match finish_reason.as_deref() {
                    Some("cancelled") => "cancelled",
                    Some("timed_out") => "timed_out",
                    _ => "completed",
                };
                let label = match status {
                    "cancelled" => "Request cancelled by user.",
                    "timed_out" => "Agent execution timed out.",
                    _ => "Final answer produced",
                };
                (
                    AgentRunEventKind::Done,
                    AgentRunPhase::Done,
                    label.to_string(),
                    Some(status.to_string()),
                )
            }
            AgentEvent::UsageUpdate { .. } => (
                AgentRunEventKind::UsageUpdated,
                AgentRunPhase::Accounting,
                "Token usage updated".to_string(),
                Some("running".to_string()),
            ),
            AgentEvent::Error { message } => (
                AgentRunEventKind::Error,
                AgentRunPhase::Done,
                message.clone(),
                Some("failed".to_string()),
            ),
            AgentEvent::AutoCompacted { .. } => (
                AgentRunEventKind::AutoCompacted,
                AgentRunPhase::Compacting,
                "Conversation context compacted".to_string(),
                Some("completed".to_string()),
            ),
            AgentEvent::ApprovalRequested { request } => (
                AgentRunEventKind::ApprovalRequested,
                AgentRunPhase::Approval,
                request.tool_name.clone(),
                Some("pending".to_string()),
            ),
            AgentEvent::ApprovalResolved { decision, .. } => (
                AgentRunEventKind::ApprovalResolved,
                AgentRunPhase::Approval,
                "Approval resolved".to_string(),
                Some(if decision.is_allowed() {
                    "allowed".to_string()
                } else {
                    "denied".to_string()
                }),
            ),
        };

        let (mut visibility, mut display_kind, mut importance) = event_presentation(kind);
        if matches!(event, AgentEvent::ControllerStatus { .. }) {
            visibility = AgentRunEventVisibility::Developer;
            importance = AgentRunEventImportance::Low;
        }
        if matches!(event, AgentEvent::PlanUpdated { .. }) {
            visibility = AgentRunEventVisibility::Developer;
        }
        if matches!(event, AgentEvent::Steering { .. }) {
            display_kind = AgentRunDisplayKind::Steering;
        }
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: String::new(),
            turn_id: String::new(),
            event_seq: 0,
            kind,
            phase,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label,
            status,
            payload: agent_event_payload(event),
            created_at: None,
        }
    }

    pub fn with_context(
        mut self,
        run_id: Option<&str>,
        turn_id: Option<&str>,
        event_seq: Option<u64>,
    ) -> Self {
        if let Some(run_id) = run_id {
            self.run_id = run_id.to_string();
        }
        if let Some(turn_id) = turn_id {
            self.turn_id = turn_id.to_string();
        }
        if let Some(event_seq) = event_seq {
            self.event_seq = event_seq;
        }
        self
    }

    pub fn with_presentation(
        mut self,
        visibility: AgentRunEventVisibility,
        display_kind: AgentRunDisplayKind,
        importance: AgentRunEventImportance,
    ) -> Self {
        self.visibility = visibility;
        self.display_kind = display_kind;
        self.importance = importance;
        self
    }

    pub fn is_durable(&self) -> bool {
        self.persistence == AgentRunEventPersistence::Durable
    }

    pub fn output_delta(
        run_id: &str,
        turn_id: Option<&str>,
        event_seq: u64,
        block_id: &str,
        channel: StreamBlockChannel,
        offset: usize,
        delta: &str,
    ) -> Self {
        let channel_label = match channel {
            StreamBlockChannel::Answer => "answer",
            StreamBlockChannel::Thinking => "thinking",
        };
        let kind = AgentRunEventKind::OutputDelta;
        let (visibility, display_kind, importance) = event_presentation(kind);
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind,
            phase: AgentRunPhase::Responding,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label: "Assistant response".to_string(),
            status: Some("streaming".to_string()),
            payload: serde_json::json!({
                "blockId": block_id,
                "channel": channel_label,
                "offset": offset,
                "delta": delta,
            }),
            created_at: None,
        }
    }

    pub fn stream_reset(run_id: &str, turn_id: Option<&str>, event_seq: u64, reason: &str) -> Self {
        let kind = AgentRunEventKind::StreamReset;
        let (visibility, display_kind, importance) = event_presentation(kind);
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind,
            phase: AgentRunPhase::Responding,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label: reason.to_string(),
            status: Some("running".to_string()),
            payload: serde_json::json!({ "reason": reason }),
            created_at: None,
        }
    }

    pub fn recovery_attempt(
        run_id: &str,
        turn_id: Option<&str>,
        event_seq: u64,
        reason: &str,
        attempt: Option<u32>,
        mode: Option<&str>,
    ) -> Self {
        let kind = AgentRunEventKind::RecoveryAttempt;
        let (visibility, display_kind, importance) = event_presentation(kind);
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind,
            phase: AgentRunPhase::Responding,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label: reason.to_string(),
            status: Some("recovering".to_string()),
            payload: serde_json::json!({
                "reason": reason,
                "attempt": attempt,
                "mode": mode,
            }),
            created_at: None,
        }
    }

    pub fn status_update(
        run_id: &str,
        turn_id: Option<&str>,
        event_seq: u64,
        phase: AgentRunPhase,
        label: &str,
        status: Option<&str>,
        payload: Option<&serde_json::Value>,
    ) -> Self {
        let mut payload_map = match payload {
            Some(serde_json::Value::Object(existing)) => existing.clone(),
            Some(existing) => {
                let mut map = serde_json::Map::new();
                map.insert("data".to_string(), existing.clone());
                map
            }
            None => serde_json::Map::new(),
        };
        payload_map
            .entry("content".to_string())
            .or_insert_with(|| serde_json::Value::String(label.to_string()));

        let kind = AgentRunEventKind::Status;
        let (visibility, display_kind, importance) = event_presentation(kind);
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind,
            phase,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label: label.to_string(),
            status: status.map(str::to_string),
            payload: serde_json::Value::Object(payload_map),
            created_at: None,
        }
    }

    pub fn terminal_error(
        run_id: &str,
        turn_id: Option<&str>,
        event_seq: u64,
        message: &str,
        status: &str,
        payload: Option<&serde_json::Value>,
    ) -> Self {
        let mut payload_map = match payload {
            Some(serde_json::Value::Object(existing)) => existing.clone(),
            Some(existing) => {
                let mut map = serde_json::Map::new();
                map.insert("data".to_string(), existing.clone());
                map
            }
            None => serde_json::Map::new(),
        };
        payload_map.insert(
            "type".to_string(),
            serde_json::Value::String("error".to_string()),
        );
        payload_map.insert(
            "message".to_string(),
            serde_json::Value::String(message.to_string()),
        );
        payload_map.insert(
            "status".to_string(),
            serde_json::Value::String(status.to_string()),
        );

        let kind = AgentRunEventKind::Error;
        let (visibility, display_kind, importance) = event_presentation(kind);
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind,
            phase: AgentRunPhase::Done,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label: message.to_string(),
            status: Some(status.to_string()),
            payload: serde_json::Value::Object(payload_map),
            created_at: None,
        }
    }

    pub fn terminal_status(
        run_id: &str,
        turn_id: Option<&str>,
        event_seq: u64,
        message: &str,
        status: &str,
        payload: Option<&serde_json::Value>,
    ) -> Self {
        let mut payload_map = match payload {
            Some(serde_json::Value::Object(existing)) => existing.clone(),
            Some(existing) => {
                let mut map = serde_json::Map::new();
                map.insert("data".to_string(), existing.clone());
                map
            }
            None => serde_json::Map::new(),
        };
        payload_map.insert(
            "message".to_string(),
            serde_json::Value::String(message.to_string()),
        );
        payload_map
            .entry("usageTotal".to_string())
            .or_insert_with(|| serde_json::json!({}));

        let kind = AgentRunEventKind::Done;
        let (visibility, display_kind, importance) = event_presentation(kind);
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind,
            phase: AgentRunPhase::Done,
            visibility,
            persistence: AgentRunEventPersistence::Durable,
            display_kind,
            importance,
            label: message.to_string(),
            status: Some(status.to_string()),
            payload: serde_json::Value::Object(payload_map),
            created_at: None,
        }
    }

    pub fn from_task_payload(payload: &serde_json::Value, fallback_run_id: &str) -> Option<Self> {
        if let Some(agent_run) = payload.get("agentRun") {
            return serde_json::from_value::<Self>(agent_run.clone()).ok();
        }

        let event_type = payload
            .get("eventType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let event_seq = payload
            .get("eventSeq")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        match event_type {
            "streamReset" => {
                let reason = payload
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Stream restarted.");
                Some(Self::stream_reset(fallback_run_id, None, event_seq, reason))
            }
            "streamBlockDelta" => {
                let block_id = payload.get("blockId")?.as_str()?;
                let channel = match payload.get("channel")?.as_str()? {
                    "thinking" => StreamBlockChannel::Thinking,
                    _ => StreamBlockChannel::Answer,
                };
                let offset = payload
                    .get("offset")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize;
                let delta = payload.get("delta")?.as_str()?;
                Some(Self::output_delta(
                    fallback_run_id,
                    None,
                    event_seq,
                    block_id,
                    channel,
                    offset,
                    delta,
                ))
            }
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.kind.is_terminal()
    }

    pub fn validate_durable_contract(&self) -> Result<(), AgentRunEventContractError> {
        if self.version != AGENT_RUN_EVENT_VERSION {
            return Err(AgentRunEventContractError::UnsupportedVersion {
                version: self.version,
                expected: AGENT_RUN_EVENT_VERSION,
            });
        }
        if self.run_id.trim().is_empty() {
            return Err(AgentRunEventContractError::MissingRunId);
        }
        if self.turn_id.trim().is_empty() {
            return Err(AgentRunEventContractError::MissingTurnId);
        }
        if self.event_seq == 0 {
            return Err(AgentRunEventContractError::MissingEventSequence);
        }

        let contract = AgentRunEventKindContract::for_kind(self.kind);
        if payload_has_all_paths(&self.payload, contract.required_payload_paths)
            || contract
                .alternative_payload_paths
                .iter()
                .any(|paths| payload_has_all_paths(&self.payload, paths))
        {
            return Ok(());
        }

        let first_missing = contract
            .required_payload_paths
            .iter()
            .copied()
            .find(|path| !payload_has_path(&self.payload, path))
            .unwrap_or_else(|| {
                contract
                    .required_payload_paths
                    .first()
                    .copied()
                    .unwrap_or("")
            });

        Err(AgentRunEventContractError::MissingPayloadField {
            kind: self.kind.as_str(),
            path: first_missing,
        })
    }
}

fn agent_event_payload(event: &AgentEvent) -> serde_json::Value {
    let canonical_tool_run = match event {
        AgentEvent::ToolCallPreparing {
            call_id, tool_name, ..
        }
        | AgentEvent::ToolCallArgsDelta {
            call_id, tool_name, ..
        } => Some(compatibility_tool_run(
            call_id,
            tool_name,
            ToolRunStatus::Preparing,
            None,
            None,
            None,
        )),
        AgentEvent::ToolCallStart {
            call_id,
            tool_name,
            arguments,
        } => Some(compatibility_tool_run(
            call_id,
            tool_name,
            ToolRunStatus::Running,
            Some(arguments),
            None,
            None,
        )),
        AgentEvent::ToolCallProgress {
            call_id,
            tool_name,
            note,
            ..
        } => Some(compatibility_tool_run(
            call_id,
            tool_name,
            ToolRunStatus::Running,
            None,
            None,
            Some(note.clone()),
        )),
        AgentEvent::ToolCallResult {
            call_id,
            tool_name,
            content,
            is_error,
            artifacts,
        } => Some(compatibility_tool_run(
            call_id,
            tool_name,
            if *is_error {
                ToolRunStatus::Failed
            } else {
                ToolRunStatus::Completed
            },
            None,
            Some((content.clone(), *is_error, artifacts.clone())),
            None,
        )),
        AgentEvent::ToolRunStarted { run }
        | AgentEvent::ToolRunUpdated { run }
        | AgentEvent::ToolRunCompleted { run } => Some(run.clone()),
        _ => None,
    };

    if let Some(run) = canonical_tool_run {
        serde_json::json!({ "run": run })
    } else {
        serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}))
    }
}

fn compatibility_tool_run(
    call_id: &str,
    tool_name: &str,
    status: ToolRunStatus,
    arguments: Option<&str>,
    result: Option<(String, bool, Option<serde_json::Value>)>,
    progress_note: Option<String>,
) -> ToolRunItem {
    let parsed_arguments = arguments
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .unwrap_or(serde_json::Value::Null);
    let registry = crate::tools::ToolRegistry::new();
    let invocation = registry.build_invocation(call_id, tool_name, parsed_arguments);
    let capabilities = invocation.capabilities;
    let (content, is_error, artifacts) = result
        .map(|(content, is_error, artifacts)| (Some(content), Some(is_error), artifacts))
        .unwrap_or((None, None, None));
    ToolRunItem {
        call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        owner: invocation.owner,
        provider_executed: false,
        status,
        arguments: arguments.map(str::to_string),
        render_kind: capabilities.render_kind,
        capabilities,
        content,
        is_error,
        artifacts,
        progress_note,
        duration_ms: None,
    }
}

fn payload_has_all_paths(payload: &serde_json::Value, paths: &[&str]) -> bool {
    paths.iter().all(|path| payload_has_path(payload, path))
}

fn payload_has_path(payload: &serde_json::Value, path: &str) -> bool {
    let mut current = payload;
    for part in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                let Some(next) = map.get(part) else {
                    return false;
                };
                current = next;
            }
            _ => return false,
        }
    }
    !current.is_null()
}

fn recovery_metadata(reason: &str) -> (AgentRunEventKind, AgentRunPhase, String, Option<String>) {
    if reason.to_ascii_lowercase().contains("retry")
        || reason.to_ascii_lowercase().contains("fallback")
        || reason.to_ascii_lowercase().contains("interrupted")
        || reason.contains("断")
    {
        (
            AgentRunEventKind::RecoveryAttempt,
            AgentRunPhase::Responding,
            reason.to_string(),
            Some("recovering".to_string()),
        )
    } else {
        (
            AgentRunEventKind::StreamReset,
            AgentRunPhase::Responding,
            reason.to_string(),
            Some("running".to_string()),
        )
    }
}

fn controller_status_phase(code: &str) -> AgentRunPhase {
    match code {
        "route_selected" => AgentRunPhase::Routing,
        "prefetch_started" | "prefetch_completed" => AgentRunPhase::Planning,
        "awaiting_user_input" => AgentRunPhase::AwaitingUserInput,
        _ => AgentRunPhase::Responding,
    }
}

fn plan_phase(phase: &str) -> AgentRunPhase {
    match phase {
        "routing" => AgentRunPhase::Routing,
        "tooling" => AgentRunPhase::Tooling,
        "approval" => AgentRunPhase::Approval,
        "awaiting_user_input" => AgentRunPhase::AwaitingUserInput,
        "compacting" => AgentRunPhase::Compacting,
        "done" => AgentRunPhase::Done,
        _ => AgentRunPhase::Planning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        ConnectionErrorCategory, ConnectionStateEvent, ConnectionStateKind, ToolRunItem,
        ToolRunStatus,
    };
    use crate::approval::{ApprovalDecision, ApprovalRequest, ApprovalRisk};
    use crate::llm::{Message, Role, Usage};
    use crate::tools::{
        ToolInputStreamingMode, ToolInterruptBehavior, ToolRenderKind, ToolRunCapabilities,
    };

    #[test]
    fn projects_agent_event_to_stable_run_event() {
        let run_event = AgentRunEvent::from_agent_event(&AgentEvent::ControllerStatus {
            code: "route_selected".to_string(),
            content: "Route selected: KnowledgeRetrieval".to_string(),
            tone: None,
        })
        .with_context(Some("run-1"), Some("turn-1"), Some(7));

        assert_eq!(run_event.version, AGENT_RUN_EVENT_VERSION);
        assert_eq!(run_event.kind, AgentRunEventKind::Status);
        assert_eq!(run_event.phase, AgentRunPhase::Routing);
        assert_eq!(run_event.visibility, AgentRunEventVisibility::Developer);
        assert_eq!(run_event.persistence, AgentRunEventPersistence::Durable);
        assert_eq!(run_event.display_kind, AgentRunDisplayKind::Status);
        assert_eq!(run_event.importance, AgentRunEventImportance::Low);
        assert_eq!(run_event.version, 2);
        assert_eq!(run_event.run_id, "run-1");
        assert_eq!(run_event.event_seq, 7);
    }

    #[test]
    fn connection_state_projects_to_user_recovery_event() {
        let run_event = AgentRunEvent::from_agent_event(&AgentEvent::ConnectionState {
            state: ConnectionStateEvent {
                state: ConnectionStateKind::Reconnecting,
                provider_id: "openai".to_string(),
                model_id: "gpt-test".to_string(),
                error_category: Some(ConnectionErrorCategory::Network),
                attempt: 1,
                max_attempts: 3,
                next_retry_at: Some("2026-08-06T10:00:00Z".to_string()),
                recoverable: true,
                queued_user_inputs: 0,
                turn_preserved: true,
            },
        });

        assert_eq!(run_event.kind, AgentRunEventKind::RecoveryAttempt);
        assert_eq!(run_event.phase, AgentRunPhase::Responding);
        assert_eq!(run_event.visibility, AgentRunEventVisibility::User);
        assert_eq!(run_event.display_kind, AgentRunDisplayKind::Recovery);
        assert_eq!(run_event.status.as_deref(), Some("reconnecting"));
        assert_eq!(run_event.payload["state"]["providerId"], "openai");
    }

    #[test]
    fn steering_presentation_is_semantic_and_label_agnostic() {
        let run_event = AgentRunEvent::from_agent_event(&AgentEvent::Steering {
            content: "focus on edge cases instead".to_string(),
        });

        assert_eq!(run_event.kind, AgentRunEventKind::Status);
        assert_eq!(run_event.display_kind, AgentRunDisplayKind::Steering);
        assert_eq!(run_event.label, "focus on edge cases instead");
        assert_eq!(run_event.status.as_deref(), Some("accepted"));
    }

    #[test]
    fn presentation_metadata_is_serialized_as_stable_wire_values() {
        let event = AgentRunEvent::status_update(
            "run-1",
            Some("turn-1"),
            1,
            AgentRunPhase::Routing,
            "Task queued",
            Some("queued"),
            None,
        )
        .with_presentation(
            AgentRunEventVisibility::Internal,
            AgentRunDisplayKind::Status,
            AgentRunEventImportance::Low,
        );

        let wire = serde_json::to_value(event).expect("run event should serialize");
        assert_eq!(wire["visibility"], "internal");
        assert_eq!(wire["persistence"], "durable");
        assert_eq!(wire["displayKind"], "status");
        assert_eq!(wire["importance"], "low");
    }

    #[test]
    fn tool_run_completion_preserves_status_and_payload() {
        let event = AgentEvent::ToolRunCompleted {
            run: ToolRunItem {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                owner: crate::plugins::capability_owner_for_tool("search_knowledge_base"),
                provider_executed: false,
                status: ToolRunStatus::Completed,
                arguments: None,
                render_kind: ToolRenderKind::Search,
                capabilities: ToolRunCapabilities {
                    input_streaming: ToolInputStreamingMode::None,
                    render_kind: ToolRenderKind::Search,
                    read_only: true,
                    destructive: false,
                    concurrency_safe: true,
                    interrupt_behavior: ToolInterruptBehavior::Block,
                    resource_keys: vec!["source:notes".to_string()],
                },
                content: Some("ok".to_string()),
                is_error: Some(false),
                artifacts: None,
                progress_note: None,
                duration_ms: Some(12),
            },
        };

        let run_event = AgentRunEvent::from_agent_event(&event);

        assert_eq!(run_event.kind, AgentRunEventKind::ToolCompleted);
        assert_eq!(run_event.status.as_deref(), Some("completed"));
        assert_eq!(
            run_event.payload["run"]["toolName"],
            "search_knowledge_base"
        );
    }

    #[test]
    fn builds_canonical_output_delta() {
        let run_event = AgentRunEvent::output_delta(
            "run-1",
            Some("turn-1"),
            9,
            "block-1",
            StreamBlockChannel::Answer,
            3,
            "abc",
        );

        assert_eq!(run_event.kind, AgentRunEventKind::OutputDelta);
        assert_eq!(run_event.payload["blockId"], "block-1");
        assert_eq!(run_event.payload["channel"], "answer");
        assert_eq!(run_event.payload["offset"], 3);
        assert_eq!(run_event.payload["delta"], "abc");
    }

    #[test]
    fn identifies_terminal_run_events() {
        let mut done_event = AgentRunEvent::from_agent_event(&AgentEvent::Status {
            content: "done".to_string(),
            tone: None,
        });
        done_event.kind = AgentRunEventKind::Done;
        let error_event = AgentRunEvent::from_agent_event(&AgentEvent::Error {
            message: "failed".to_string(),
        });
        let status_event = AgentRunEvent::from_agent_event(&AgentEvent::Status {
            content: "working".to_string(),
            tone: None,
        });

        assert!(done_event.is_terminal());
        assert!(error_event.is_terminal());
        assert!(!status_event.is_terminal());
    }

    #[test]
    fn done_event_preserves_cancelled_finish_reason() {
        let run_event = AgentRunEvent::from_agent_event(&AgentEvent::Done {
            message: Message::text(Role::Assistant, "Request cancelled by user."),
            usage_total: Usage::default(),
            last_prompt_tokens: 0,
            context_breakdown: None,
            cached: false,
            finish_reason: Some("cancelled".to_string()),
        });

        assert_eq!(run_event.kind, AgentRunEventKind::Done);
        assert_eq!(run_event.status.as_deref(), Some("cancelled"));
        assert_eq!(run_event.label, "Request cancelled by user.");
    }

    #[test]
    fn builds_canonical_status_update() {
        let payload = serde_json::json!({ "detail": "queued" });
        let run_event = AgentRunEvent::status_update(
            "run-1",
            Some("turn-1"),
            3,
            AgentRunPhase::Routing,
            "Task queued",
            Some("queued"),
            Some(&payload),
        );

        assert_eq!(run_event.version, AGENT_RUN_EVENT_VERSION);
        assert_eq!(run_event.kind, AgentRunEventKind::Status);
        assert_eq!(run_event.event_seq, 3);
        assert_eq!(run_event.status.as_deref(), Some("queued"));
        assert_eq!(run_event.payload["detail"], "queued");
        assert_eq!(run_event.payload["content"], "Task queued");
        run_event.validate_durable_contract().unwrap();
    }

    #[test]
    fn parses_wire_values_for_storage_round_trips() {
        for kind in AgentRunEventKind::ALL {
            assert_eq!(AgentRunEventKind::from_wire(kind.as_str()), Some(kind));
        }

        for phase in [
            AgentRunPhase::Routing,
            AgentRunPhase::Planning,
            AgentRunPhase::Responding,
            AgentRunPhase::Tooling,
            AgentRunPhase::Approval,
            AgentRunPhase::AwaitingUserInput,
            AgentRunPhase::Compacting,
            AgentRunPhase::Accounting,
            AgentRunPhase::Done,
        ] {
            assert_eq!(AgentRunPhase::from_wire(phase.as_str()), Some(phase));
        }

        assert_eq!(AgentRunEventKind::from_wire("missing"), None);
        assert_eq!(AgentRunPhase::from_wire("missing"), None);
    }

    #[test]
    fn terminal_error_preserves_non_failed_status() {
        let payload = serde_json::json!({ "reason": "timeout" });
        let run_event = AgentRunEvent::terminal_error(
            "run-1",
            Some("turn-1"),
            11,
            "Agent execution timed out.",
            "timed_out",
            Some(&payload),
        );

        assert_eq!(run_event.version, AGENT_RUN_EVENT_VERSION);
        assert_eq!(run_event.kind, AgentRunEventKind::Error);
        assert_eq!(run_event.phase, AgentRunPhase::Done);
        assert_eq!(run_event.event_seq, 11);
        assert_eq!(run_event.status.as_deref(), Some("timed_out"));
        assert_eq!(run_event.payload["type"], "error");
        assert_eq!(run_event.payload["message"], "Agent execution timed out.");
        assert_eq!(run_event.payload["reason"], "timeout");
    }

    #[test]
    fn every_event_kind_has_a_durable_contract() {
        for kind in AgentRunEventKind::ALL {
            let contract = AgentRunEventKindContract::for_kind(kind);
            assert_eq!(contract.kind, kind);
            assert!(
                !contract.required_payload_paths.is_empty(),
                "{} should declare required payload fields",
                kind.as_str()
            );
        }
    }

    #[test]
    fn durable_contract_rejects_missing_context() {
        let event = AgentRunEvent::from_agent_event(&AgentEvent::Status {
            content: "working".to_string(),
            tone: None,
        });

        assert_eq!(
            event.validate_durable_contract().unwrap_err(),
            AgentRunEventContractError::MissingRunId
        );
    }

    #[test]
    fn agent_event_variants_project_to_valid_durable_contracts() {
        let approval_request = ApprovalRequest::new(
            "approval-1",
            "run_shell",
            &serde_json::json!({ "command": "echo ok" }),
            ApprovalRisk::High,
            "test approval",
        );
        let events = vec![
            AgentEvent::TextDelta {
                delta: "a".to_string(),
            },
            AgentEvent::StreamBlockDelta {
                block_id: "block-1".to_string(),
                channel: StreamBlockChannel::Answer,
                offset: 0,
                delta: "b".to_string(),
            },
            AgentEvent::StreamReset {
                reason: "provider stream reset".to_string(),
            },
            AgentEvent::StreamReset {
                reason: "retry after provider disconnect".to_string(),
            },
            AgentEvent::ToolCallPreparing {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                args_bytes: 12,
                index: 0,
            },
            AgentEvent::ToolCallStart {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                arguments: "{}".to_string(),
            },
            AgentEvent::ToolCallArgsDelta {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                arguments_delta: "{\"q\"".to_string(),
                index: 0,
            },
            AgentEvent::ToolCallProgress {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                note: "searching".to_string(),
                activity: None,
            },
            AgentEvent::ToolCallResult {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                content: "ok".to_string(),
                is_error: false,
                artifacts: None,
            },
            AgentEvent::ToolRunStarted {
                run: sample_tool_run(ToolRunStatus::Running),
            },
            AgentEvent::ToolRunUpdated {
                run: sample_tool_run(ToolRunStatus::Running),
            },
            AgentEvent::ToolRunCompleted {
                run: sample_tool_run(ToolRunStatus::Completed),
            },
            AgentEvent::Thinking {
                content: "thinking".to_string(),
            },
            AgentEvent::Status {
                content: "Route selected: Direct".to_string(),
                tone: None,
            },
            AgentEvent::ConnectionState {
                state: ConnectionStateEvent {
                    state: ConnectionStateKind::Reconnecting,
                    provider_id: "openai".to_string(),
                    model_id: "gpt-test".to_string(),
                    error_category: Some(ConnectionErrorCategory::Network),
                    attempt: 1,
                    max_attempts: 3,
                    next_retry_at: None,
                    recoverable: true,
                    queued_user_inputs: 0,
                    turn_preserved: true,
                },
            },
            AgentEvent::PlanUpdated {
                plan: serde_json::json!({ "items": [] }),
                phase: Some("planning".to_string()),
                summary: Some("planned".to_string()),
            },
            AgentEvent::Done {
                message: Message::text(Role::Assistant, "done"),
                usage_total: Usage::default(),
                last_prompt_tokens: 0,
                context_breakdown: None,
                cached: false,
                finish_reason: Some("stop".to_string()),
            },
            AgentEvent::UsageUpdate {
                usage_total: Usage::default(),
                last_prompt_tokens: 42,
                context_breakdown: None,
            },
            AgentEvent::Error {
                message: "failed".to_string(),
            },
            AgentEvent::AutoCompacted { evicted_count: 2 },
            AgentEvent::ApprovalRequested {
                request: approval_request.clone(),
            },
            AgentEvent::ApprovalResolved {
                request_id: approval_request.id.clone(),
                decision: ApprovalDecision::Deny,
            },
        ];

        for (index, event) in events.into_iter().enumerate() {
            let run_event = AgentRunEvent::from_agent_event(&event).with_context(
                Some("run-1"),
                Some("turn-1"),
                Some(index as u64 + 1),
            );
            run_event.validate_durable_contract().unwrap_or_else(|err| {
                panic!(
                    "projected {:?} into invalid {:?}: {err}",
                    event, run_event.kind
                )
            });
            if matches!(
                run_event.kind,
                AgentRunEventKind::ToolPreparing
                    | AgentRunEventKind::ToolStarted
                    | AgentRunEventKind::ToolProgress
                    | AgentRunEventKind::ToolCompleted
            ) {
                assert!(
                    run_event.payload.get("run").is_some_and(serde_json::Value::is_object),
                    "public tool RunEvents must expose exactly one canonical payload.run shape: {event:?}"
                );
            }
        }
    }

    #[test]
    fn historical_v2_flat_tool_payloads_remain_valid_on_read() {
        let cases = [
            (
                AgentRunEventKind::ToolPreparing,
                serde_json::json!({
                    "callId": "call-1",
                    "toolName": "read_file",
                    "argsBytes": 12,
                    "index": 0
                }),
            ),
            (
                AgentRunEventKind::ToolStarted,
                serde_json::json!({
                    "callId": "call-1",
                    "toolName": "read_file",
                    "arguments": "{}"
                }),
            ),
            (
                AgentRunEventKind::ToolProgress,
                serde_json::json!({
                    "callId": "call-1",
                    "note": "reading"
                }),
            ),
            (
                AgentRunEventKind::ToolCompleted,
                serde_json::json!({
                    "callId": "call-1",
                    "toolName": "read_file",
                    "content": "ok",
                    "isError": false
                }),
            ),
        ];

        for (index, (kind, payload)) in cases.into_iter().enumerate() {
            let event = AgentRunEvent {
                version: AGENT_RUN_EVENT_VERSION,
                run_id: "historical-run".to_string(),
                turn_id: "historical-turn".to_string(),
                event_seq: index as u64 + 1,
                kind,
                phase: AgentRunPhase::Tooling,
                visibility: AgentRunEventVisibility::User,
                persistence: AgentRunEventPersistence::Durable,
                display_kind: AgentRunDisplayKind::Tool,
                importance: AgentRunEventImportance::Normal,
                label: "read_file".to_string(),
                status: Some("completed".to_string()),
                payload,
                created_at: None,
            };
            event.validate_durable_contract().unwrap_or_else(|error| {
                panic!("historical {} event was rejected: {error}", kind.as_str())
            });
        }
    }

    fn sample_tool_run(status: ToolRunStatus) -> ToolRunItem {
        ToolRunItem {
            call_id: "call-1".to_string(),
            tool_name: "search_knowledge_base".to_string(),
            owner: crate::plugins::capability_owner_for_tool("search_knowledge_base"),
            provider_executed: false,
            status,
            arguments: Some("{}".to_string()),
            render_kind: ToolRenderKind::Search,
            capabilities: ToolRunCapabilities {
                input_streaming: ToolInputStreamingMode::None,
                render_kind: ToolRenderKind::Search,
                read_only: true,
                destructive: false,
                concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Block,
                resource_keys: vec!["source:notes".to_string()],
            },
            content: Some("ok".to_string()),
            is_error: Some(false),
            artifacts: None,
            progress_note: Some("searching".to_string()),
            duration_ms: Some(12),
        }
    }
}
