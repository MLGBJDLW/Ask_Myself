use serde::{Deserialize, Serialize};

use super::context::ContextUsageBreakdown;
use crate::approval::{ApprovalDecision, ApprovalRequest};
use crate::llm::{Message, Usage};
use crate::plugins::ToolPluginInfo;
use crate::tools::{ToolRenderKind, ToolRunCapabilities};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreamBlockChannel {
    Answer,
    Thinking,
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
    /// Clear partial stream output before replaying a recovered response.
    StreamReset { reason: String },
    /// The model has started assembling a tool call, but the arguments are not
    /// stable enough to render or execute yet.
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
    /// A tool call is about to be executed with complete arguments.
    ToolCallStart {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        arguments: String,
    },
    /// Legacy incremental fragment of tool-call arguments streamed mid-response.
    ///
    /// Generic tools should not rely on this because partial JSON arguments are
    /// often syntactically invalid. The main agent loop now emits
    /// `ToolCallPreparing` while arguments are still being assembled, then
    /// `ToolCallStart` once the complete argument string is available.
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
    /// Heartbeat emitted while a long-running tool is still executing.
    ///
    /// Used to keep the frontend watchdog alive for long-running tools.
    ToolCallProgress {
        #[serde(rename = "callId")]
        call_id: String,
        note: String,
    },
    /// Result of a tool execution.
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
    pub plugin: ToolPluginInfo,
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
