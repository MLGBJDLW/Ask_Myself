//! LLM provider types and traits for the agent framework.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

pub mod anthropic;
pub mod google;
pub mod ollama;
pub mod openai;
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

/// Reasoning effort level for OpenAI o-series models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
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
    /// OpenAI o-series reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Provider type hint — lets providers apply model-specific logic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<ProviderType>,
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
    /// HTTP request timeout in seconds. When `None`, the provider's built-in
    /// default (usually 300 s) is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Supported LLM provider backends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderType {
    OpenAi,
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

    /// Quick connectivity / auth check.
    async fn health_check(&self) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create a provider instance from configuration.
pub fn create_provider(config: ProviderConfig) -> Result<Box<dyn LlmProvider>, CoreError> {
    match config.provider_type {
        ProviderType::OpenAi
        | ProviderType::DeepSeek
        | ProviderType::LmStudio
        | ProviderType::AzureOpenAi
        | ProviderType::Zhipu
        | ProviderType::Moonshot
        | ProviderType::Qwen
        | ProviderType::Doubao
        | ProviderType::Yi
        | ProviderType::Baichuan
        | ProviderType::Custom => Ok(Box::new(openai::OpenAiProvider::new(config)?)),
        ProviderType::Anthropic => Ok(Box::new(anthropic::AnthropicProvider::new(config)?)),
        ProviderType::Google => Ok(Box::new(google::GeminiProvider::new(config)?)),
        ProviderType::Ollama => Ok(Box::new(ollama::OllamaProvider::new(config)?)),
    }
}

/// Determines whether a model is known to support vision/image inputs.
/// Conservative: returns `false` for unknown models.
pub fn model_supports_vision(provider_type: &ProviderType, model: &str) -> bool {
    let m = model.to_lowercase();
    match provider_type {
        ProviderType::OpenAi | ProviderType::AzureOpenAi => {
            m.contains("gpt-4o")
                || m.contains("gpt-4-turbo")
                || m.contains("gpt-4-vision")
                || m.contains("gpt-4.1")
                || m.starts_with("o1")
                || m.starts_with("o3")
                || m.starts_with("o4")
        }
        ProviderType::Anthropic => m.contains("claude-3") || m.contains("claude-4"),
        ProviderType::Google => true,
        ProviderType::DeepSeek => false,
        ProviderType::Zhipu => m.contains("glm-4v"),
        ProviderType::Qwen => m.contains("qwen-vl"),
        ProviderType::Moonshot
        | ProviderType::Doubao
        | ProviderType::Yi
        | ProviderType::Baichuan => false,
        ProviderType::Ollama | ProviderType::LmStudio => {
            m.contains("vision")
                || m.contains("llava")
                || m.contains("bakllava")
                || m.contains("moondream")
                || m.contains("cogvlm")
                || m.contains("minicpm")
        }
        ProviderType::Custom => {
            m.contains("vision") || m.contains("gpt-4o") || m.contains("claude-3")
        }
    }
}
