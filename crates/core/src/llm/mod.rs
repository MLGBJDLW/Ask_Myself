//! LLM provider types and traits for the agent framework.

use async_trait::async_trait;
use futures::{stream::BoxStream, Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::provider_catalog::model_supports_vision_from_catalog;
use crate::provider_registry::{provider_adapter_for_type, ProviderAdapterKind};

pub mod anthropic;
pub mod google;
pub mod ollama;
pub mod openai;
mod prompt_cache;
pub mod streaming;

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
        }
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
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Max,
    XHigh,
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
    /// Provider-specific reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Provider type hint — lets providers apply model-specific logic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<ProviderType>,
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
            reasoning_effort: None,
            provider_type: None,
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
    /// Provider-side prompt-cache tokens read for this request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    /// Provider-side prompt-cache tokens missed for this request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_miss_tokens: Option<u32>,
    /// Provider-side prompt-cache tokens written/created for this request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Other,
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

/// Provider-normalized stream event.
///
/// Existing providers still implement `stream()` in terms of `StreamChunk`.
/// This adapter Interface gives the agent layer a seam for provider-level
/// recovery policy without requiring every provider to change at once.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProviderStreamEvent {
    Chunk { chunk: StreamChunk },
    RecoverableError { message: String },
    Cancelled { message: String },
    TerminalError { message: String },
}

/// Convert a legacy chunk stream into provider-normalized stream events.
pub fn stream_chunks_to_provider_events<'a>(
    stream: BoxStream<'a, Result<StreamChunk, CoreError>>,
) -> BoxStream<'a, ProviderStreamEvent> {
    Box::pin(stream.map(provider_stream_event_from_chunk_result))
}

fn provider_stream_event_from_chunk_result(
    item: Result<StreamChunk, CoreError>,
) -> ProviderStreamEvent {
    match item {
        Ok(chunk) => ProviderStreamEvent::Chunk { chunk },
        Err(CoreError::StreamIncomplete(message) | CoreError::TransientLlm(message)) => {
            ProviderStreamEvent::RecoverableError { message }
        }
        Err(CoreError::Cancelled(message)) => ProviderStreamEvent::Cancelled { message },
        Err(error) => ProviderStreamEvent::TerminalError {
            message: error.to_string(),
        },
    }
}

/// Incremental tool call data received during streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDelta {
    pub id: String,
    pub name: Option<String>,
    /// Partial JSON arguments appended incrementally.
    pub arguments_delta: String,
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
    Doubao,
    Yi,
    Baichuan,
    Custom,
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// Trait implemented by each LLM backend.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Human-readable provider name (e.g. "OpenAI").
    fn name(&self) -> &str;

    /// List available models from this provider.
    async fn list_models(&self) -> Result<Vec<String>, CoreError>;

    /// Send a completion request and return the full response.
    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError>;

    /// Send a completion request and return a stream of chunks.
    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError>;

    /// Provider-level stream Interface with normalized error classification.
    async fn stream_events(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, ProviderStreamEvent>, CoreError> {
        let stream = self.stream(request).await?;
        Ok(stream_chunks_to_provider_events(stream))
    }

    /// Quick connectivity / auth check.
    async fn health_check(&self) -> Result<(), CoreError>;
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

/// Create a provider instance from configuration.
pub fn create_provider(mut config: ProviderConfig) -> Result<Box<dyn LlmProvider>, CoreError> {
    config.base_url = normalize_base_url(config.base_url);

    match provider_adapter_for_type(config.provider_type) {
        ProviderAdapterKind::OpenAiCompatible => Ok(Box::new(openai::OpenAiProvider::new(config)?)),
        ProviderAdapterKind::Anthropic => Ok(Box::new(anthropic::AnthropicProvider::new(config)?)),
        ProviderAdapterKind::Google => Ok(Box::new(google::GeminiProvider::new(config)?)),
        ProviderAdapterKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(config)?)),
    }
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
        assert!(!model_supports_vision(
            &ProviderType::DeepSeek,
            "deepseek-v4-pro"
        ));
        assert!(model_supports_vision(&ProviderType::Qwen, "qwen3-vl-plus"));
        assert!(!model_supports_vision(&ProviderType::Qwen, "qwen3.6-plus"));
        assert!(model_supports_vision(
            &ProviderType::LmStudio,
            "local-vision-model"
        ));
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
            ProviderStreamEvent::TerminalError { message }
                if message == "LLM error: provider refused request"
        ));
    }
}
