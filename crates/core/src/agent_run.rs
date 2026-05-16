//! Stable run protocol for agent execution.
//!
//! `AgentEvent` is optimized for the live executor loop. `AgentRunEvent` is the
//! durable Interface for replay, task timelines, and future clients.

use serde::{Deserialize, Serialize};

use crate::agent::{AgentEvent, StreamBlockChannel, ToolRunStatus};

pub const AGENT_RUN_EVENT_VERSION: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunPhase {
    Routing,
    Planning,
    Responding,
    Tooling,
    Approval,
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
            Self::Compacting => "compacting",
            Self::Accounting => "accounting",
            Self::Done => "done",
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

    pub fn task_event_type(self) -> &'static str {
        match self {
            Self::OutputDelta | Self::Thinking | Self::UsageUpdated => "stream",
            Self::StreamReset | Self::Status | Self::AutoCompacted | Self::Done => "status",
            Self::PlanUpdated => "plan",
            Self::ToolPreparing | Self::ToolStarted | Self::ToolProgress | Self::ToolCompleted => {
                "tool"
            }
            Self::ApprovalRequested | Self::ApprovalResolved => "approval",
            Self::RecoveryAttempt => "status",
            Self::Error => "error",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
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
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub payload: serde_json::Value,
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
            AgentEvent::ToolCallProgress { note, .. } => (
                AgentRunEventKind::ToolProgress,
                AgentRunPhase::Tooling,
                note.clone(),
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
                status_phase(content),
                content.clone(),
                tone.clone().or_else(|| Some("running".to_string())),
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
            AgentEvent::Done { .. } => (
                AgentRunEventKind::Done,
                AgentRunPhase::Done,
                "Final answer produced".to_string(),
                Some("completed".to_string()),
            ),
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

        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: String::new(),
            turn_id: String::new(),
            event_seq: 0,
            kind,
            phase,
            label,
            status,
            payload: serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({})),
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
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind: AgentRunEventKind::OutputDelta,
            phase: AgentRunPhase::Responding,
            label: "Assistant response".to_string(),
            status: Some("streaming".to_string()),
            payload: serde_json::json!({
                "blockId": block_id,
                "channel": channel_label,
                "offset": offset,
                "delta": delta,
            }),
        }
    }

    pub fn stream_reset(run_id: &str, turn_id: Option<&str>, event_seq: u64, reason: &str) -> Self {
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind: AgentRunEventKind::StreamReset,
            phase: AgentRunPhase::Responding,
            label: reason.to_string(),
            status: Some("running".to_string()),
            payload: serde_json::json!({ "reason": reason }),
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
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind: AgentRunEventKind::RecoveryAttempt,
            phase: AgentRunPhase::Responding,
            label: reason.to_string(),
            status: Some("recovering".to_string()),
            payload: serde_json::json!({
                "reason": reason,
                "attempt": attempt,
                "mode": mode,
            }),
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
        Self {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: run_id.to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
            event_seq,
            kind: AgentRunEventKind::Status,
            phase,
            label: label.to_string(),
            status: status.map(str::to_string),
            payload: payload.cloned().unwrap_or_else(|| serde_json::json!({})),
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

    pub fn task_event_type(&self) -> &'static str {
        self.kind.task_event_type()
    }

    pub fn is_terminal(&self) -> bool {
        self.kind.is_terminal()
    }
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

fn status_phase(content: &str) -> AgentRunPhase {
    if content.starts_with("Route selected: ") {
        AgentRunPhase::Routing
    } else {
        AgentRunPhase::Responding
    }
}

fn plan_phase(phase: &str) -> AgentRunPhase {
    match phase {
        "routing" => AgentRunPhase::Routing,
        "tooling" => AgentRunPhase::Tooling,
        "approval" => AgentRunPhase::Approval,
        "compacting" => AgentRunPhase::Compacting,
        "done" => AgentRunPhase::Done,
        _ => AgentRunPhase::Planning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ToolRunItem, ToolRunStatus};
    use crate::tools::{
        ToolInputStreamingMode, ToolInterruptBehavior, ToolRenderKind, ToolRunCapabilities,
    };

    #[test]
    fn projects_agent_event_to_stable_run_event() {
        let run_event = AgentRunEvent::from_agent_event(&AgentEvent::Status {
            content: "Route selected: KnowledgeRetrieval".to_string(),
            tone: None,
        })
        .with_context(Some("run-1"), Some("turn-1"), Some(7));

        assert_eq!(run_event.version, AGENT_RUN_EVENT_VERSION);
        assert_eq!(run_event.kind, AgentRunEventKind::Status);
        assert_eq!(run_event.phase, AgentRunPhase::Routing);
        assert_eq!(run_event.task_event_type(), "status");
        assert_eq!(run_event.version, 2);
        assert_eq!(run_event.run_id, "run-1");
        assert_eq!(run_event.event_seq, 7);
    }

    #[test]
    fn tool_run_completion_preserves_status_and_payload() {
        let event = AgentEvent::ToolRunCompleted {
            run: ToolRunItem {
                call_id: "call-1".to_string(),
                tool_name: "search_knowledge_base".to_string(),
                plugin: crate::plugins::plugin_for_tool("search_knowledge_base"),
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
        assert_eq!(run_event.task_event_type(), "stream");
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
        assert_eq!(run_event.task_event_type(), "status");
        assert_eq!(run_event.event_seq, 3);
        assert_eq!(run_event.status.as_deref(), Some("queued"));
        assert_eq!(run_event.payload["detail"], "queued");
    }
}
