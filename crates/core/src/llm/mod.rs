//! LLM provider types and traits for the agent framework.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{stream::BoxStream, Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::provider_catalog::model_supports_vision_from_catalog;
use crate::provider_registry::{provider_adapter_for_type, ProviderAdapterKind};

pub mod anthropic;
pub mod fallback;
pub mod google;
pub mod message_validation;
pub mod native_search;
pub mod ollama;
pub mod openai;
pub mod prompt_cache;
pub(crate) mod provider_boundary;
pub mod provider_turn;
pub mod reasoning_profile;
pub mod reasoning_replay;
pub mod streaming;
pub(crate) mod transport;

// ---------------------------------------------------------------------------
// Core message types
// ---------------------------------------------------------------------------

/// Role of a message participant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Provider-neutral stability of a prompt segment. Wire adapters use this
/// metadata to place supported cache boundaries without inferring intent from
/// a message role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PromptStability {
    Stable,
    Replayable,
    Volatile,
}

/// Lifetime of compiler-owned prompt material within one agent turn.
///
/// `PromptStability` controls provider caching; it does not say whether a
/// volatile instruction is enduring turn scaffolding or a replaceable
/// per-sample directive. Keeping that distinction typed prevents answer-only
/// projection from discarding route, orchestration, and workflow constraints.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PromptLifetime {
    #[default]
    Turn,
    Step,
}

impl PromptLifetime {
    fn is_turn(&self) -> bool {
        *self == Self::Turn
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum CacheBoundaryHint {
    PolicyEnd,
    StableEvidenceEnd,
    ReplayableTurnTail,
    LatestToolRound,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheHint {
    pub stability: PromptStability,
    pub boundary: CacheBoundaryHint,
    #[serde(default, skip_serializing_if = "PromptLifetime::is_turn")]
    pub lifetime: PromptLifetime,
}

/// A single part of a multimodal message content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentPart {
    /// Plain text content.
    #[serde(rename = "text")]
    Text { text: String },
    /// Base64-encoded image data.
    #[serde(rename = "image")]
    Image {
        /// MIME type (e.g., "image/jpeg", "image/png", "image/webp", "image/gif")
        media_type: String,
        /// Base64-encoded image data
        data: String,
    },
    /// Provider-native replay sidecar. It is never rendered as user-visible
    /// content; wire adapters consume it only when the route snapshot matches.
    #[serde(rename = "providerTurn")]
    ProviderTurn {
        envelope: Box<provider_turn::ProviderTurnEnvelope>,
    },
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: Role,
    pub parts: Vec<ContentPart>,
    /// Optional name for tool messages (the tool-call id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls requested by the assistant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallRequest>>,
    /// Provider-specific assistant reasoning content to pass back in
    /// multi-step tool loops (e.g. DeepSeek `reasoning_content`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Internal prompt-compiler metadata. Provider adapters consume this
    /// sidecar and never include it in wire message content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_hint: Option<PromptCacheHint>,
}

impl Message {
    /// Create a text-only message.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![ContentPart::Text {
                text: content.into(),
            }],
            name: None,
            tool_calls: None,
            reasoning_content: None,
            prompt_cache_hint: None,
        }
    }

    /// Create a text message with a name.
    pub fn text_with_name(role: Role, content: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![ContentPart::Text {
                text: content.into(),
            }],
            name: Some(name.into()),
            tool_calls: None,
            reasoning_content: None,
            prompt_cache_hint: None,
        }
    }

    pub fn with_prompt_cache_hint(
        mut self,
        stability: PromptStability,
        boundary: CacheBoundaryHint,
    ) -> Self {
        self.prompt_cache_hint = Some(PromptCacheHint {
            stability,
            boundary,
            lifetime: PromptLifetime::Turn,
        });
        self
    }

    pub fn with_prompt_lifetime(mut self, lifetime: PromptLifetime) -> Self {
        if let Some(hint) = self.prompt_cache_hint.as_mut() {
            hint.lifetime = lifetime;
        }
        self
    }

    pub fn prompt_cache_hint(&self) -> Option<(PromptStability, CacheBoundaryHint)> {
        self.prompt_cache_hint
            .map(|hint| (hint.stability, hint.boundary))
    }

    pub fn prompt_lifetime(&self) -> PromptLifetime {
        self.prompt_cache_hint
            .map(|hint| hint.lifetime)
            .unwrap_or_default()
    }

    /// Get the combined text content from all text parts.
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Check if this message has any image parts.
    pub fn has_images(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p, ContentPart::Image { .. }))
    }

    /// Get all image parts.
    pub fn image_parts(&self) -> Vec<&ContentPart> {
        self.parts
            .iter()
            .filter(|p| matches!(p, ContentPart::Image { .. }))
            .collect()
    }

    pub fn provider_turn(&self) -> Option<&provider_turn::ProviderTurnEnvelope> {
        self.parts.iter().find_map(|part| match part {
            ContentPart::ProviderTurn { envelope } => Some(envelope.as_ref()),
            _ => None,
        })
    }

    pub fn set_provider_turn(&mut self, envelope: provider_turn::ProviderTurnEnvelope) {
        self.clear_provider_turn();
        self.parts.push(ContentPart::ProviderTurn {
            envelope: Box::new(envelope),
        });
    }

    /// Remove provider-native replay state when a history repair changes the
    /// assistant/tool envelope it authenticated.
    pub fn clear_provider_turn(&mut self) {
        self.parts
            .retain(|part| !matches!(part, ContentPart::ProviderTurn { .. }));
        if let Some(tool_calls) = self.tool_calls.as_mut() {
            for tool_call in tool_calls {
                tool_call.thought_signature = None;
            }
        }
    }

    /// Remove secret-adjacent provider replay state before serializing a
    /// message to the desktop UI or another display-only consumer.
    pub fn without_provider_turn(mut self) -> Self {
        self.clear_provider_turn();
        self
    }
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Provider-specific reasoning effort level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Max,
    XHigh,
    Ultra,
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Minimal => write!(f, "minimal"),
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Max => write!(f, "max"),
            Self::XHigh => write!(f, "xhigh"),
            Self::Ultra => write!(f, "ultra"),
        }
    }
}

impl ReasoningEffort {
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "max" => Some(Self::Max),
            "xhigh" => Some(Self::XHigh),
            "ultra" => Some(Self::Ultra),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request sent to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// Anthropic extended thinking budget (token count).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    /// Explicit user intent for providers whose reasoning mode is controlled
    /// independently from effort or budget. `None` preserves the provider's
    /// documented default instead of inventing a cross-provider fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_enabled: Option<bool>,
    /// Provider-specific reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Provider type hint — lets providers apply model-specific logic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<ProviderType>,
    /// Privacy-preserving provider routing key. It is transport metadata, not
    /// user-visible prompt content, and is currently consumed by OpenRouter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_session_id: Option<String>,
    /// When true, hint to the provider that multiple tool_use blocks in one
    /// response are allowed. Default: true. Providers that natively support
    /// parallel function calling translate this into a wire-level flag
    /// (e.g. OpenAI `parallel_tool_calls`, Anthropic
    /// `tool_choice.disable_parallel_tool_use: false`).
    #[serde(default = "default_parallel_tool_calls")]
    pub parallel_tool_calls: bool,
}

fn default_parallel_tool_calls() -> bool {
    true
}

impl Default for CompletionRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: None,
            routing_session_id: None,
            parallel_tool_calls: true,
        }
    }
}

/// Definition of a tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool parameters.
    pub parameters: serde_json::Value,
}

/// Response from an LLM provider (non-streaming).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResponse {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallRequest>>,
    pub finish_reason: FinishReason,
    pub usage: Usage,
    /// Thinking / chain-of-thought text (if the model supports it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Exact provider-native replay state for this assistant turn. This is an
    /// internal protocol sidecar and must never be rendered as visible text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_replay: Option<provider_turn::ProviderReplayPayload>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    /// Tokens consumed by model thinking/reasoning (if supported).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<u32>,
    /// Tokens attributed to function/tool declarations by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_prompt_tokens: Option<u32>,
    /// Provider-side prompt-cache tokens read for this request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    /// Provider-side prompt-cache tokens missed for this request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_miss_tokens: Option<u32>,
    /// Provider-side prompt-cache tokens written/created for this request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    /// Scrubbed provider usage and routing fragment retained beside normalized
    /// counters. It must never contain prompt or completion content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_raw: Option<serde_json::Value>,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    /// Provider-owned server-tool turn must be resumed with provider-native
    /// assistant state (for example Anthropic `pause_turn`).
    ProviderPause,
    /// The provider exhausted the full model context rather than only the
    /// per-request output allowance.
    ContextLimit,
    /// The provider explicitly rejected or could not construct a tool call.
    MalformedToolCall,
    /// A response ended without the terminal/item lifecycle required by its
    /// declared wire dialect.
    ProtocolIncomplete,
    /// A raw terminal value unknown to this adapter. It is retained for
    /// diagnostics and must never be treated as a natural stop.
    Unknown(String),
    /// Legacy compatibility for persisted values that predate raw terminal
    /// preservation. New provider adapters should use `Unknown(raw)`.
    Other,
}

impl FinishReason {
    pub fn allows_completed_client_tools(&self) -> bool {
        matches!(self, Self::Stop | Self::ToolCalls)
    }

    pub fn raw_reason(&self) -> Option<&str> {
        match self {
            Self::Unknown(reason) => Some(reason.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

/// A single chunk from a streaming response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamChunk {
    /// Incremental text content (may be empty when tool-call deltas arrive).
    pub delta: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_delta: Option<ToolCallDelta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Thinking text delta (streamed chain-of-thought).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_delta: Option<String>,
}

/// Lifecycle status for a tool that the provider executes inside its own
/// Responses request. These events are presentation and trace data only: the
/// agent must never dispatch them through Nexa's local tool registry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderHostedToolStatus {
    Running,
    Completed,
    Failed,
}

/// Provider-owned Responses item family. Keep this separate from the remote
/// tool's display name (notably for named MCP calls) so presentation does not
/// depend on Nexa's local tool registry recognizing that name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderHostedToolKind {
    WebSearch,
    FileSearch,
    CodeInterpreter,
    ComputerUse,
    ImageGeneration,
    Mcp,
    Shell,
}

/// Provider-neutral projection of a provider-executed Responses item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHostedToolEvent {
    pub call_id: String,
    pub tool_name: String,
    pub kind: ProviderHostedToolKind,
    pub provider_id: String,
    pub status: ProviderHostedToolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<serde_json::Value>,
}

/// Terminal failure reported through the canonical provider-event stream.
///
/// Chunk-only wire protocols are normalized into provider events in one
/// direction. Provider-owned events, such as hosted tools, are never collapsed
/// back into chunks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProviderStreamFailure {
    Provider { message: String },
    RateLimited { retry_after_secs: u64 },
    ContextOverflow { prompt_tokens: u32, max_tokens: u32 },
    Internal { message: String },
}

impl ProviderStreamFailure {
    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider {
            message: message.into(),
        }
    }

    pub fn into_core_error(self) -> CoreError {
        match self {
            Self::Provider { message } => CoreError::Llm(message),
            Self::RateLimited { retry_after_secs } => CoreError::RateLimited { retry_after_secs },
            Self::ContextOverflow {
                prompt_tokens,
                max_tokens,
            } => CoreError::ContextOverflow(prompt_tokens, max_tokens),
            Self::Internal { message } => CoreError::Internal(message),
        }
    }
}

impl From<CoreError> for ProviderStreamFailure {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Llm(message) => Self::Provider { message },
            CoreError::RateLimited { retry_after_secs } => Self::RateLimited { retry_after_secs },
            CoreError::ContextOverflow(prompt_tokens, max_tokens) => Self::ContextOverflow {
                prompt_tokens,
                max_tokens,
            },
            CoreError::Internal(message) => Self::Internal { message },
            error => Self::Internal {
                message: error.to_string(),
            },
        }
    }
}

/// Canonical incremental event emitted by every LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProviderStreamEvent {
    Chunk {
        chunk: Box<StreamChunk>,
    },
    HostedTool {
        tool: Box<ProviderHostedToolEvent>,
    },
    /// Terminal provider-native replay state for the accepted sample. Unlike
    /// tool deltas, it authorizes no execution and is never user-visible.
    ReplayState {
        replay: Box<provider_turn::ProviderReplayPayload>,
    },
    RecoverableError {
        message: String,
    },
    Cancelled {
        message: String,
    },
    TerminalError {
        failure: ProviderStreamFailure,
    },
}

/// Normalize a chunk-only provider wire stream into canonical provider events.
pub fn stream_chunks_to_provider_events<'a>(
    stream: BoxStream<'a, Result<StreamChunk, CoreError>>,
) -> BoxStream<'a, ProviderStreamEvent> {
    Box::pin(stream.map(provider_stream_event_from_chunk_result))
}

#[cfg(test)]
pub(crate) fn provider_events_from_chunk_stream<'a>(
    stream: BoxStream<'a, Result<StreamChunk, CoreError>>,
) -> Result<BoxStream<'a, ProviderStreamEvent>, CoreError> {
    Ok(stream_chunks_to_provider_events(stream))
}

fn provider_stream_event_from_chunk_result(
    item: Result<StreamChunk, CoreError>,
) -> ProviderStreamEvent {
    match item {
        Ok(chunk) => ProviderStreamEvent::Chunk {
            chunk: Box::new(chunk),
        },
        Err(error) => provider_stream_event_from_error(error),
    }
}

pub(crate) fn provider_stream_event_from_error(error: CoreError) -> ProviderStreamEvent {
    match error {
        CoreError::StreamIncomplete(message) | CoreError::TransientLlm(message) => {
            ProviderStreamEvent::RecoverableError { message }
        }
        CoreError::Cancelled(message) => ProviderStreamEvent::Cancelled { message },
        error => ProviderStreamEvent::TerminalError {
            failure: error.into(),
        },
    }
}

/// Incremental tool call data received during streaming.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ToolCallArgumentsDeltaKind {
    #[default]
    Fragment,
    Snapshot,
}

/// Provider-neutral tool argument payload with runtime-local assembly semantics.
///
/// Most providers send opaque string fragments. Some OpenAI-compatible SSE
/// endpoints instead send a complete JSON object on every update; retaining
/// that distinction prevents two valid root snapshots from being concatenated.
/// The kind is runtime-local because provider replay persists only the final
/// verified tool request, while the text keeps the existing serialized wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ToolCallArgumentsDelta {
    text: String,
    #[serde(skip)]
    kind: ToolCallArgumentsDeltaKind,
}

impl ToolCallArgumentsDelta {
    pub fn snapshot(text: String) -> Self {
        Self {
            text,
            kind: ToolCallArgumentsDeltaKind::Snapshot,
        }
    }

    pub fn is_snapshot(&self) -> bool {
        self.kind == ToolCallArgumentsDeltaKind::Snapshot
    }
}

impl std::ops::Deref for ToolCallArgumentsDelta {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl AsRef<str> for ToolCallArgumentsDelta {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

impl From<String> for ToolCallArgumentsDelta {
    fn from(text: String) -> Self {
        Self {
            text,
            kind: ToolCallArgumentsDeltaKind::Fragment,
        }
    }
}

impl From<&str> for ToolCallArgumentsDelta {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

impl PartialEq<&str> for ToolCallArgumentsDelta {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDelta {
    pub id: String,
    pub name: Option<String>,
    /// Opaque JSON fragment or a typed complete-object snapshot.
    pub arguments_delta: ToolCallArgumentsDelta,
    /// Optional tool-call index from providers that stream multiple calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider configuration
// ---------------------------------------------------------------------------

/// Configuration for connecting to an LLM provider.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStreamingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_max_retries: Option<u32>,
}

impl ProviderStreamingConfig {
    pub fn stream_idle_timeout(self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.stream_idle_timeout_ms
                .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT.as_millis() as u64)
                .clamp(1_000, 3_600_000),
        )
    }

    pub fn connect_timeout(self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.connect_timeout_ms
                .unwrap_or(10_000)
                .clamp(1_000, 300_000),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    /// Base URL override (required for Custom / self-hosted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// API key (not serialized to prevent accidental leaking).
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    /// Organisation / project header (OpenAI, Azure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// HTTP request timeout in seconds. Non-streaming requests use it for the
    /// full request. Streaming requests use it only until response headers are
    /// received. Active streams use a separate idle timeout so healthy long
    /// outputs are not capped by total duration. 0 disables the startup timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub streaming: ProviderStreamingConfig,
}

pub(crate) fn configured_request_timeout(timeout_secs: u64) -> Option<std::time::Duration> {
    (timeout_secs > 0).then(|| std::time::Duration::from_secs(timeout_secs))
}

pub(crate) const DEFAULT_STREAM_IDLE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(300);

pub(crate) fn with_request_timeout(
    builder: reqwest::RequestBuilder,
    timeout: Option<std::time::Duration>,
) -> reqwest::RequestBuilder {
    match timeout {
        Some(timeout) => builder.timeout(timeout),
        None => builder,
    }
}

/// Serialize a provider request exactly once and retain it in a cheaply
/// cloneable byte buffer for retries and request-size telemetry.
pub(crate) fn serialized_json_body<T: Serialize>(
    value: &T,
    context: &str,
) -> Result<Bytes, CoreError> {
    serde_json::to_vec(value)
        .map(Bytes::from)
        .map_err(|error| CoreError::Llm(format!("Failed to serialize {context}: {error}")))
}

pub(crate) async fn send_stream_start_request(
    builder: reqwest::RequestBuilder,
    timeout: Option<std::time::Duration>,
    context: &str,
) -> Result<reqwest::Response, CoreError> {
    let send = builder.send();
    let result = match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, send).await {
            Ok(result) => result,
            Err(_) => {
                return Err(CoreError::TransientLlm(format!(
                    "{context} timed out after {}s before the stream started",
                    timeout.as_secs()
                )));
            }
        },
        None => send.await,
    };

    result.map_err(|e| {
        let message = format!("{context} failed: {e}");
        if e.is_connect() || e.is_timeout() {
            CoreError::TransientLlm(message)
        } else {
            CoreError::Llm(message)
        }
    })
}

pub(crate) async fn next_stream_item_with_idle_timeout<S, T, E>(
    stream: &mut S,
    idle_timeout: std::time::Duration,
    context: &str,
) -> Result<Option<Result<T, E>>, CoreError>
where
    S: Stream<Item = Result<T, E>> + Unpin,
{
    if idle_timeout.is_zero() {
        return Ok(stream.next().await);
    }

    match tokio::time::timeout(idle_timeout, stream.next()).await {
        Ok(item) => Ok(item),
        Err(_) => Err(CoreError::StreamIncomplete(format!(
            "{context} was idle for {}s",
            idle_timeout.as_secs()
        ))),
    }
}

/// Supported LLM provider backends.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderType {
    OpenAi,
    OpenRouter,
    Anthropic,
    Google,
    DeepSeek,
    Ollama,
    LmStudio,
    AzureOpenAi,
    Zhipu,
    Moonshot,
    Qwen,
    AlibabaModelStudio,
    SiliconFlow,
    Doubao,
    Yi,
    Baichuan,
    Custom,
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// Ownership of replay-history projection for one provider invocation.
///
/// Most concrete providers expose a stable route before invocation and let
/// the caller project history for that route. Route-selecting adapters must
/// instead receive the original history and project only after they choose a
/// concrete route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayHistoryProjection {
    Caller(reasoning_profile::ReasoningReplayPolicy),
    ProviderSelectedRoute,
}

/// Trait implemented by each LLM backend.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Human-readable provider name (e.g. "OpenAI").
    fn name(&self) -> &str;

    /// Optional provider-scoped retry budget for both stream startup and
    /// replay-safe disconnect recovery. `None` keeps the runtime defaults.
    fn stream_max_retries(&self) -> Option<u32> {
        None
    }

    /// Resolved cache capability for this concrete provider endpoint. Agent
    /// diagnostics consume the same profile as the wire adapter.
    fn prompt_cache_profile(&self, model: &str) -> prompt_cache::PromptCacheProfile {
        prompt_cache::resolve_prompt_cache_profile(
            ProviderType::Custom,
            None,
            prompt_cache::PromptCacheApiStyle::Local,
            model,
        )
    }

    /// Endpoint-scoped contract for replaying provider reasoning. The agent
    /// uses this before dispatching tool calls; callers never infer it from a
    /// model name alone.
    fn reasoning_replay_policy(&self, _model: &str) -> reasoning_profile::ReasoningReplayPolicy {
        reasoning_profile::ReasoningReplayPolicy::Unknown
    }

    /// Policy used to project history before a provider request is opened.
    /// Providers that select among heterogeneous routes may defer projection
    /// until the concrete route is known.
    fn reasoning_replay_history_policy(
        &self,
        model: &str,
    ) -> reasoning_profile::ReasoningReplayPolicy {
        self.reasoning_replay_policy(model)
    }

    /// Declare which side owns replay-history projection for this invocation.
    /// The typed contract avoids overloading `NotRequired` as an adapter
    /// sentinel: ordinary no-replay routes still need compatibility filtering.
    fn replay_history_projection(&self, request: &CompletionRequest) -> ReplayHistoryProjection {
        if request.reasoning_enabled == Some(false)
            || request.reasoning_effort == Some(ReasoningEffort::None)
        {
            ReplayHistoryProjection::Caller(reasoning_profile::ReasoningReplayPolicy::NotRequired)
        } else {
            ReplayHistoryProjection::Caller(self.reasoning_replay_history_policy(&request.model))
        }
    }

    /// Immutable identity of the concrete route that will receive this
    /// request. Fallback adapters delegate this to their selected route.
    fn route_snapshot(&self, request: &CompletionRequest) -> provider_turn::RouteSnapshot {
        provider_turn::RouteSnapshot::unknown(
            self.name(),
            &request.model,
            if request.reasoning_enabled == Some(false)
                || request.reasoning_effort == Some(ReasoningEffort::None)
            {
                reasoning_profile::ReasoningReplayPolicy::NotRequired
            } else {
                self.reasoning_replay_policy(&request.model)
            },
        )
    }

    /// List available models from this provider.
    async fn list_models(&self) -> Result<Vec<String>, CoreError>;

    /// Send a completion request and return the full response.
    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError>;

    /// Open the canonical provider-event stream for this request.
    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError>;

    /// Quick connectivity / auth check.
    async fn health_check(&self) -> Result<(), CoreError>;

    /// Optional privacy-safe runtime metadata for durable agent traces.
    async fn runtime_metadata(&self) -> Option<serde_json::Value> {
        None
    }
}

struct MessageValidatingProvider {
    inner: Box<dyn LlmProvider>,
}

impl MessageValidatingProvider {
    fn new(inner: Box<dyn LlmProvider>) -> Self {
        Self { inner }
    }

    fn validate(&self, request: &CompletionRequest) -> Result<(), CoreError> {
        message_validation::validate_provider_request_with_context(
            &request.messages,
            self.inner.name(),
            &request.model,
            request.routing_session_id.as_deref(),
            None,
        )
    }
}

#[async_trait]
impl LlmProvider for MessageValidatingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn stream_max_retries(&self) -> Option<u32> {
        self.inner.stream_max_retries()
    }

    fn prompt_cache_profile(&self, model: &str) -> prompt_cache::PromptCacheProfile {
        self.inner.prompt_cache_profile(model)
    }

    fn reasoning_replay_policy(&self, model: &str) -> reasoning_profile::ReasoningReplayPolicy {
        self.inner.reasoning_replay_policy(model)
    }

    fn reasoning_replay_history_policy(
        &self,
        model: &str,
    ) -> reasoning_profile::ReasoningReplayPolicy {
        self.inner.reasoning_replay_history_policy(model)
    }

    fn replay_history_projection(&self, request: &CompletionRequest) -> ReplayHistoryProjection {
        self.inner.replay_history_projection(request)
    }

    fn route_snapshot(&self, request: &CompletionRequest) -> provider_turn::RouteSnapshot {
        self.inner.route_snapshot(request)
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        self.inner.list_models().await
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        self.validate(request)?;
        self.inner.complete(request).await
    }

    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        self.validate(request)?;
        self.inner.stream_events(request).await
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        self.inner.health_check().await
    }

    async fn runtime_metadata(&self) -> Option<serde_json::Value> {
        self.inner.runtime_metadata().await
    }
}

fn normalize_base_url(base_url: Option<String>) -> Option<String> {
    base_url.and_then(|url| {
        let trimmed = url.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

fn provider_adapter_for_config(config: &ProviderConfig) -> ProviderAdapterKind {
    if provider_boundary::is_deepseek_anthropic_endpoint(
        config.provider_type,
        config.base_url.as_deref(),
    ) {
        ProviderAdapterKind::Anthropic
    } else {
        provider_adapter_for_type(config.provider_type)
    }
}

/// Create a provider instance from configuration.
pub fn create_provider(mut config: ProviderConfig) -> Result<Box<dyn LlmProvider>, CoreError> {
    config.base_url = normalize_base_url(config.base_url);

    let adapter = provider_adapter_for_config(&config);
    let provider: Box<dyn LlmProvider> = match adapter {
        ProviderAdapterKind::OpenAiCompatible => Box::new(openai::OpenAiProvider::new(config)?),
        ProviderAdapterKind::Anthropic => Box::new(anthropic::AnthropicProvider::new(config)?),
        ProviderAdapterKind::Google => Box::new(google::GeminiProvider::new(config)?),
        ProviderAdapterKind::Ollama => Box::new(ollama::OllamaProvider::new(config)?),
    };
    Ok(Box::new(MessageValidatingProvider::new(provider)))
}

/// Whether the adapter must obtain the complete response before it can expose
/// a synthetic stream. Streaming-only deadlines must not wrap this mode.
pub fn provider_uses_non_streaming_fallback(provider_type: ProviderType, model: &str) -> bool {
    (provider_type == ProviderType::OpenRouter)
        || (matches!(
            provider_adapter_for_type(provider_type),
            ProviderAdapterKind::OpenAiCompatible
        ) && openai::requires_non_streaming_fallback(model))
}

/// Determines whether a model is expected to support vision/image inputs.
/// Defaults to `true` for most modern models; only returns `false` for models
/// known to lack vision support (text-only, embedding-only, older generations).
pub fn model_supports_vision(provider_type: &ProviderType, model: &str) -> bool {
    if let Some(supports_vision) = model_supports_vision_from_catalog(*provider_type, model) {
        return supports_vision;
    }

    let m = model.to_lowercase();
    match provider_type {
        ProviderType::OpenAi | ProviderType::AzureOpenAi => {
            // Deny: older text-only models
            !(m.contains("gpt-3.5") || m.contains("text-davinci") || m.contains("text-embedding"))
        }
        ProviderType::OpenRouter => {
            // OpenRouter serves a mixed catalog; prefer the shared catalog when
            // known, otherwise allow modern multimodal families by default.
            !(m.contains("text-embedding") || m.contains("embedding"))
        }
        ProviderType::Anthropic => {
            // Deny: pre-Claude-3 models
            !(m.contains("claude-2") || m.contains("claude-instant"))
        }
        ProviderType::Google => true,
        ProviderType::DeepSeek => false,
        ProviderType::Zhipu => {
            // Most models support vision; deny embedding/cogview
            !(m.contains("embedding") || m.contains("cogview"))
        }
        ProviderType::Qwen => {
            // Most models support vision; deny embedding/text-only
            !(m.contains("embedding") || m.contains("text"))
        }
        ProviderType::AlibabaModelStudio => {
            m.contains("vl") || m.contains("vision") || m.starts_with("kimi-k2.6")
        }
        ProviderType::Moonshot => {
            // Deny old moonshot-v1-* text-only models
            !m.starts_with("moonshot-v1")
        }
        ProviderType::Doubao => {
            // Most models support vision; deny embedding
            !m.contains("embedding")
        }
        ProviderType::Yi => {
            // Most models support vision; deny embedding/text-only
            !(m.contains("embedding") || m.contains("text"))
        }
        ProviderType::Baichuan => {
            // Most models support vision; deny embedding/text-only
            !(m.contains("embedding") || m.contains("text"))
        }
        ProviderType::SiliconFlow => {
            m.contains("vl") || m.contains("vision") || m.contains("glm-4.5v")
        }
        ProviderType::Ollama | ProviderType::LmStudio => {
            // Local models: allow if name hints at vision capability
            m.contains("vision")
                || m.contains("llava")
                || m.contains("bakllava")
                || m.contains("moondream")
                || m.contains("cogvlm")
                || m.contains("minicpm")
                || m.contains("-vl")
                || m.contains("internvl")
        }
        ProviderType::Custom => {
            // Custom/OpenRouter: default to true unless clearly text-only
            !(m.contains("gpt-3.5") || m.contains("text-davinci") || m.contains("text-embedding"))
        }
    }
}

/// Strict image-input eligibility for privacy-sensitive routing. Unknown
/// models are deliberately ineligible: heuristics may improve legacy UX, but
/// they must never authorize sending raw image bytes to an undeclared model.
pub fn model_declares_vision_support(provider_type: &ProviderType, model: &str) -> bool {
    model_supports_vision_from_catalog(*provider_type, model).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};

    fn text_chunk(delta: &str) -> StreamChunk {
        StreamChunk {
            delta: delta.to_string(),
            tool_call_delta: None,
            finish_reason: None,
            usage: None,
            thinking_delta: None,
        }
    }

    #[test]
    fn vision_support_prefers_provider_catalog_then_fallback() {
        assert!(model_supports_vision(&ProviderType::OpenAi, "gpt-5.5"));
        assert!(model_supports_vision(
            &ProviderType::OpenAi,
            "muse-spark-1.3"
        ));
        assert!(model_supports_vision(
            &ProviderType::Google,
            "gemini-3.8-flash"
        ));
        assert!(!model_supports_vision(
            &ProviderType::DeepSeek,
            "deepseek-v4-pro"
        ));
        assert!(model_supports_vision(&ProviderType::Qwen, "qwen3-vl-plus"));
        assert!(model_supports_vision(&ProviderType::Qwen, "qwen3.6-plus"));
        assert!(!model_supports_vision(
            &ProviderType::AlibabaModelStudio,
            "deepseek-v4-pro"
        ));
        assert!(model_supports_vision(
            &ProviderType::AlibabaModelStudio,
            "kimi-k2.6"
        ));
        assert!(model_supports_vision(
            &ProviderType::SiliconFlow,
            "zai-org/GLM-4.5V"
        ));
        assert!(model_supports_vision(
            &ProviderType::LmStudio,
            "local-vision-model"
        ));
    }

    #[test]
    fn strict_vision_eligibility_fails_closed_for_unknown_models() {
        assert!(!model_declares_vision_support(
            &ProviderType::OpenAi,
            "unknown-private-text-model"
        ));
        assert!(model_declares_vision_support(
            &ProviderType::OpenAi,
            "gpt-5.5"
        ));
    }

    #[test]
    fn serialized_request_bytes_are_reused_without_copying_the_payload() {
        let body = serde_json::json!({"model": "gpt-test", "messages": ["hello"]});
        let bytes = serialized_json_body(&body, "test request").expect("serialize request");
        let retry = bytes.clone();

        assert_eq!(bytes, serde_json::to_vec(&body).unwrap());
        assert_eq!(bytes.as_ptr(), retry.as_ptr());
    }

    #[test]
    fn openai_compatible_completion_only_models_report_non_streaming_fallback() {
        assert!(provider_uses_non_streaming_fallback(
            ProviderType::OpenAi,
            "gpt-5.5-pro"
        ));
        assert!(provider_uses_non_streaming_fallback(
            ProviderType::OpenRouter,
            "gpt-5.5-pro-preview"
        ));
        assert!(provider_uses_non_streaming_fallback(
            ProviderType::OpenRouter,
            "moonshotai/kimi-k3"
        ));
        assert!(provider_uses_non_streaming_fallback(
            ProviderType::OpenRouter,
            "x-ai/grok-4.6"
        ));
        assert!(!provider_uses_non_streaming_fallback(
            ProviderType::OpenAi,
            "gpt-5.5"
        ));
        assert!(!provider_uses_non_streaming_fallback(
            ProviderType::Anthropic,
            "gpt-5.5-pro"
        ));
    }

    #[test]
    fn deepseek_anthropic_route_selects_the_block_adapter_only_at_exact_endpoint() {
        let config = ProviderConfig {
            provider_type: ProviderType::DeepSeek,
            api_key: Some("test".to_string()),
            base_url: Some("https://api.deepseek.com/anthropic".to_string()),
            org_id: None,
            timeout_secs: None,
            streaming: Default::default(),
        };
        assert_eq!(
            provider_adapter_for_config(&config),
            ProviderAdapterKind::Anthropic
        );
        assert_eq!(
            provider_adapter_for_config(&ProviderConfig {
                base_url: Some("https://proxy.example.com/anthropic".to_string()),
                ..config
            }),
            ProviderAdapterKind::OpenAiCompatible
        );
    }

    #[test]
    fn provider_streaming_defaults_keep_deepseek_idle_at_five_minutes() {
        let config = ProviderStreamingConfig::default();
        assert_eq!(
            config.stream_idle_timeout(),
            std::time::Duration::from_secs(300)
        );
        assert_eq!(config.connect_timeout(), std::time::Duration::from_secs(10));

        let overridden = ProviderStreamingConfig {
            stream_idle_timeout_ms: Some(420_000),
            connect_timeout_ms: Some(25_000),
            stream_max_retries: Some(3),
        };
        assert_eq!(
            overridden.stream_idle_timeout(),
            std::time::Duration::from_secs(420)
        );
        assert_eq!(
            overridden.connect_timeout(),
            std::time::Duration::from_secs(25)
        );
    }

    #[tokio::test]
    async fn next_stream_item_with_idle_timeout_reports_recoverable_idle_stream() {
        let mut stream = futures::stream::pending::<Result<&'static str, ()>>();

        let result = next_stream_item_with_idle_timeout(
            &mut stream,
            std::time::Duration::from_millis(10),
            "test stream",
        )
        .await;

        assert!(matches!(
            result,
            Err(CoreError::StreamIncomplete(message))
                if message.contains("test stream") && message.contains("idle")
        ));
    }

    #[tokio::test]
    async fn stream_chunk_adapter_classifies_stream_incomplete_as_recoverable() {
        let source = Box::pin(stream::iter(vec![
            Ok(text_chunk("hello")),
            Err(CoreError::StreamIncomplete("connection closed".to_string())),
        ]));

        let events = stream_chunks_to_provider_events(source)
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            &events[0],
            ProviderStreamEvent::Chunk { chunk } if chunk.delta == "hello"
        ));
        assert!(matches!(
            &events[1],
            ProviderStreamEvent::RecoverableError { message } if message == "connection closed"
        ));
    }

    #[tokio::test]
    async fn stream_chunk_adapter_classifies_transient_llm_as_recoverable() {
        let source = Box::pin(stream::iter(vec![Err(CoreError::TransientLlm(
            "temporary network failure".to_string(),
        ))]));

        let events = stream_chunks_to_provider_events(source)
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            &events[0],
            ProviderStreamEvent::RecoverableError { message }
                if message == "temporary network failure"
        ));
    }

    #[tokio::test]
    async fn stream_chunk_adapter_classifies_cancelled_separately() {
        let source = Box::pin(stream::iter(vec![Err(CoreError::Cancelled(
            "user stopped request".to_string(),
        ))]));

        let events = stream_chunks_to_provider_events(source)
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            &events[0],
            ProviderStreamEvent::Cancelled { message } if message == "user stopped request"
        ));
    }

    #[tokio::test]
    async fn stream_chunk_adapter_classifies_other_errors_as_terminal() {
        let source = Box::pin(stream::iter(vec![Err(CoreError::Llm(
            "provider refused request".to_string(),
        ))]));

        let events = stream_chunks_to_provider_events(source)
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            &events[0],
            ProviderStreamEvent::TerminalError {
                failure: ProviderStreamFailure::Provider { message }
            } if message == "provider refused request"
        ));
    }

    #[test]
    fn terminal_stream_failure_round_trips_without_duplicating_llm_prefix() {
        let failure = ProviderStreamFailure::from(CoreError::Llm(
            "Responses function_call contained incomplete arguments".to_string(),
        ));

        let error = failure.into_core_error();

        assert_eq!(
            error.to_string(),
            "LLM error: Responses function_call contained incomplete arguments"
        );
    }

    #[test]
    fn provider_trait_exposes_only_the_canonical_event_stream() {
        let source = include_str!("mod.rs");
        let trait_source = source
            .split_once("pub trait LlmProvider")
            .expect("provider trait declaration")
            .1
            .split_once("struct MessageValidatingProvider")
            .expect("provider trait boundary")
            .0;

        assert_eq!(trait_source.matches("async fn stream_events(").count(), 1);
        assert!(!trait_source.contains("async fn stream("));
    }
}
