//! Stable run protocol for agent execution.
//!
//! `AgentEvent` is optimized for the live executor loop. `AgentRunEvent` is the
//! durable Interface for replay, task timelines, and future clients.

use serde::{Deserialize, Serialize};

use crate::agent::{AgentEvent, ToolRunStatus};

pub const AGENT_RUN_EVENT_VERSION: u16 = 1;

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
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunEvent {
    pub version: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<u64>,
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
            AgentEvent::StreamReset { reason } => (
                AgentRunEventKind::StreamReset,
                AgentRunPhase::Responding,
                reason.clone(),
                Some("running".to_string()),
            ),
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
            run_id: None,
            turn_id: None,
            event_seq: None,
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
        self.run_id = run_id.map(ToString::to_string);
        self.turn_id = turn_id.map(ToString::to_string);
        self.event_seq = event_seq;
        self
    }

    pub fn task_event_type(&self) -> &'static str {
        self.kind.task_event_type()
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
        assert_eq!(run_event.run_id.as_deref(), Some("run-1"));
        assert_eq!(run_event.event_seq, Some(7));
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
}
