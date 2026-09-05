use serde::{Deserialize, Serialize};

use super::context::ContextUsageBreakdown;
use crate::activity::ActivityEvent;
use crate::approval::{ApprovalDecision, ApprovalRequest};
use crate::llm::{Message, Usage};
use crate::plugins::CapabilityOwner;
use crate::tools::{ToolRenderKind, ToolRunCapabilities};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreamBlockChannel {
    Answer,
    Thinking,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStateKind {
    Degraded,
    Reconnecting,
    Recovered,
    Offline,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionErrorCategory {
    Network,
    Timeout,
    RateLimit,
    ProviderUnavailable,
    Authentication,
    Unknown,
}

/// Structured provider connectivity state for recoverable turns.
///
/// This event is deliberately separate from model reasoning and user-visible
/// answer text so retry status can never be persisted as assistant content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStateEvent {
    pub state: ConnectionStateKind,
    pub provider_id: String,
    pub model_id: String,
    pub error_category: Option<ConnectionErrorCategory>,
    pub attempt: u32,
    pub max_attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    pub recoverable: bool,
    pub queued_user_inputs: u32,
    pub turn_preserved: bool,
}

/// Events emitted by the agent during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    /// Incremental text token from the LLM.
    TextDelta { delta: String },
    /// Ordered, authoritative delta for a typed stream block.
    StreamBlockDelta {
        #[serde(rename = "blockId")]
        block_id: String,
        channel: StreamBlockChannel,
        /// Starting UTF-8 byte offset within this block.
        offset: usize,
        delta: String,
    },
    /// Replace one output block from an authoritative full record.
    StreamBlockSnapshot {
        #[serde(rename = "blockId")]
        block_id: String,
        channel: StreamBlockChannel,
        text: String,
    },
    /// Reset stream projection before a recovery or controller restart.
    StreamReset {
        reason: String,
        /// True when the current answer/thinking/preparing-tool sample was
        /// never accepted for execution and must be removed, not retained as
        /// a cancelled historical round.
        #[serde(default)]
        discard_sample: bool,
    },
    /// Retired serialized compatibility envelope. New producers emit a
    /// `ToolRunStarted` item with `Preparing` status instead.
    ToolCallPreparing {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "argsBytes")]
        args_bytes: u32,
        /// Tool-call index when the provider streams multiple calls in parallel.
        index: u32,
    },
    /// Retired serialized compatibility envelope. New producers emit
    /// `ToolRunUpdated` with complete arguments instead.
    ToolCallStart {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        arguments: String,
    },
    /// Retired serialized compatibility envelope for an argument fragment.
    /// Generic tools never consume or produce partial JSON arguments.
    ToolCallArgsDelta {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "argumentsDelta")]
        arguments_delta: String,
        /// Tool-call index when the provider streams multiple calls in parallel.
        index: u32,
    },
    /// Retired serialized compatibility envelope. Heartbeats and activity now
    /// use `ToolRunUpdated`.
    ToolCallProgress {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        note: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        activity: Option<ActivityEvent>,
    },
    /// Retired serialized compatibility envelope. New producers emit
    /// `ToolRunCompleted`.
    ToolCallResult {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        content: String,
        #[serde(rename = "isError")]
        is_error: bool,
        artifacts: Option<serde_json::Value>,
    },
    /// Canonical lifecycle item for a tool run.
    ToolRunStarted { run: ToolRunItem },
    /// Authoritative update for an in-flight tool run.
    ToolRunUpdated { run: ToolRunItem },
    /// Authoritative final state for a tool run.
    ToolRunCompleted { run: ToolRunItem },
    /// Thinking / chain-of-thought text (if the model supports it).
    Thinking { content: String },
    /// A lightweight status update for the trace timeline.
    Status {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tone: Option<String>,
    },
    /// Controller/runtime telemetry. This is developer-visible and uses a
    /// semantic code so phase projection never depends on localized text.
    ControllerStatus {
        code: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tone: Option<String>,
    },
    /// Provider connectivity and retry state. Never treat this as reasoning.
    ConnectionState { state: ConnectionStateEvent },
    /// A user-authored steering message accepted by the active turn.
    ///
    /// This is distinct from `Status` so presentation never depends on a
    /// localized label or text prefix.
    Steering { content: String },
    /// Updated typed execution plan for the active task run.
    PlanUpdated {
        plan: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// The agent finished producing a final answer.
    Done {
        message: Message,
        #[serde(rename = "usageTotal")]
        usage_total: Usage,
        /// The prompt token count from the *last* LLM iteration (best
        /// represents how full the context window currently is).
        #[serde(rename = "lastPromptTokens")]
        last_prompt_tokens: u32,
        #[serde(rename = "contextBreakdown", skip_serializing_if = "Option::is_none")]
        context_breakdown: Option<ContextUsageBreakdown>,
        /// Whether this response came from the answer cache.
        #[serde(default)]
        cached: bool,
        /// Why the model stopped generating (e.g. "stop", "length", "content_filter").
        #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
        finish_reason: Option<String>,
    },
    /// Intermediate token usage update emitted after each LLM iteration.
    UsageUpdate {
        #[serde(rename = "usageTotal")]
        usage_total: Usage,
        #[serde(rename = "lastPromptTokens")]
        last_prompt_tokens: u32,
        #[serde(rename = "contextBreakdown", skip_serializing_if = "Option::is_none")]
        context_breakdown: Option<ContextUsageBreakdown>,
    },
    /// An error occurred during execution.
    Error { message: String },
    /// The agent auto-compacted the conversation to free context space.
    AutoCompacted {
        /// Number of messages that were summarized.
        #[serde(rename = "evictedCount")]
        evicted_count: usize,
    },
    /// A high-risk tool call is waiting for user approval via the GUI.
    ///
    /// The UI should render a dialog and later resolve the request by
    /// invoking the `approve_tool_call_cmd` Tauri command.
    ApprovalRequested { request: ApprovalRequest },
    /// A previously emitted approval request was resolved (for UI cleanup).
    ApprovalResolved {
        #[serde(rename = "requestId")]
        request_id: String,
        decision: ApprovalDecision,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolRunStatus {
    Preparing,
    ApprovalPending,
    Running,
    Completed,
    Failed,
    Declined,
    Cancelled,
    TimedOut,
}

impl ToolRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::ApprovalPending => "approval_pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRunItem {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub owner: CapabilityOwner,
    /// True when the upstream provider executed the tool inside the model
    /// request. Provider-executed runs are display/trace events and must not be
    /// submitted to Nexa's local tool dispatcher.
    #[serde(default, rename = "providerExecuted")]
    pub provider_executed: bool,
    pub status: ToolRunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    #[serde(rename = "renderKind")]
    pub render_kind: ToolRenderKind,
    pub capabilities: ToolRunCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<serde_json::Value>,
    #[serde(rename = "progressNote", skip_serializing_if = "Option::is_none")]
    pub progress_note: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}
