use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::activity::ActivityState;
use crate::conversation::ConversationMessage;
use crate::llm::{LlmProvider, ProviderType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionPhase {
    Queued,
    Planning,
    Summarizing,
    Validating,
    Committing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionPolicy {
    pub provider_attempt_timeout_ms: u64,
    pub total_deadline_ms: u64,
    pub max_retries: u32,
}

impl Default for ContextCompactionPolicy {
    fn default() -> Self {
        Self {
            provider_attempt_timeout_ms: 45_000,
            total_deadline_ms: 75_000,
            max_retries: 1,
        }
    }
}

impl ContextCompactionPolicy {
    pub fn normalized(self) -> Self {
        Self {
            provider_attempt_timeout_ms: self.provider_attempt_timeout_ms.clamp(1_000, 120_000),
            total_deadline_ms: self.total_deadline_ms.clamp(2_000, 180_000),
            max_retries: self.max_retries.min(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartContextCompactionRequest {
    pub conversation_id: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub policy: ContextCompactionPolicy,
}

pub struct ContextCompactionJob {
    pub request: StartContextCompactionRequest,
    pub snapshot_version: String,
    pub model: String,
    pub context_window: Option<u32>,
    pub max_response_tokens: u32,
    pub provider_type: Option<ProviderType>,
    pub provider_label: String,
    pub summarizer: Arc<dyn LlmProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionHandle {
    pub operation_id: String,
    pub conversation_id: String,
    pub snapshot_version: String,
    pub state: ActivityState,
    pub phase: ContextCompactionPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionResult {
    pub conversation_id: String,
    pub checkpoint_id: Option<String>,
    pub messages_before: usize,
    pub messages_after: usize,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub evicted_messages: usize,
    pub summary_kind: String,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextProjection {
    pub messages: Vec<ConversationMessage>,
    pub checkpoint_id: Option<String>,
    pub projected: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextCheckpointInput {
    pub operation_id: String,
    pub conversation_id: String,
    pub idempotency_key: String,
    pub snapshot_high_watermark: i64,
    pub snapshot_hash: String,
    pub summary: String,
    pub retained_tail_message_ids: Vec<String>,
    pub retained_start_sort_order: i64,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub provider: String,
    pub model: String,
    pub usage: Option<serde_json::Value>,
}
