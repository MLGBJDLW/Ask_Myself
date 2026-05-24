//! OpenAI-compatible LLM provider.
//!
//! Also used for DeepSeek, LM Studio, Azure OpenAI, and custom endpoints
//! that expose the same `/v1/chat/completions` interface.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::{
    configured_request_timeout, send_stream_start_request, streaming::parse_sse_stream,
    with_request_timeout, CompletionRequest, CompletionResponse, ContentPart, FinishReason,
    LlmProvider, Message, ProviderConfig, ProviderType, ReasoningEffort, Role, StreamChunk,
    ToolCallRequest, ToolDefinition, Usage,
};
use crate::error::CoreError;
use crate::provider_catalog::model_supports_reasoning_from_catalog;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT_SECS: u64 = 600;
const MAX_COMPLETE_ATTEMPTS: u32 = 3;
const MISSING_REASONING_CONTENT_PLACEHOLDER: &str =
    "[reasoning content unavailable in local history]";

// ---------------------------------------------------------------------------
// OpenAI API wire types — request
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<OaiThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OaiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OaiStreamOptions>,
}

#[derive(Serialize)]
struct OaiStreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct OaiThinking {
    #[serde(rename = "type")]
    thinking_type: String,
}

#[derive(Serialize)]
struct OaiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OaiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCallOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

/// OpenAI content: either a plain string or an array of content parts.
#[derive(Serialize)]
#[serde(untagged)]
enum OaiContent {
    Text(String),
    Parts(Vec<OaiContentPart>),
}

/// A single part in the OpenAI content array format.
#[derive(Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum OaiContentPart {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<OaiCacheControl>,
    },
    ImageUrl {
        image_url: OaiImageUrl,
    },
}

#[derive(Serialize)]
struct OaiImageUrl {
    url: String,
}

#[derive(Serialize)]
struct OaiToolCallOut {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OaiFunctionOut,
}

#[derive(Serialize)]
struct OaiFunctionOut {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiToolFunction,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<OaiCacheControl>,
}

#[derive(Serialize)]
struct OaiToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct OaiCacheControl {
    r#type: String,
}

// ---------------------------------------------------------------------------
// OpenAI API wire types — response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OaiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OaiToolCallIn>>,
    reasoning_content: Option<String>,
}

#[derive(Deserialize)]
struct OaiToolCallIn {
    id: String,
    function: OaiFunctionIn,
}

#[derive(Deserialize)]
struct OaiFunctionIn {
    name: String,
    arguments: OaiArgumentsIn,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OaiArgumentsIn {
    Text(String),
    Json(serde_json::Value),
}

impl OaiArgumentsIn {
    fn into_argument_string(self) -> String {
        match self {
            OaiArgumentsIn::Text(text) => text,
            OaiArgumentsIn::Json(value) => {
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
            }
        }
    }
}

#[derive(Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    completion_tokens_details: Option<OaiCompletionTokensDetails>,
    #[serde(default)]
    prompt_tokens_details: Option<OaiPromptTokensDetails>,
}

#[derive(Deserialize)]
struct OaiCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct OaiPromptTokensDetails {
    #[serde(default, alias = "cache_read_input_tokens", alias = "cachedTokens")]
    cached_tokens: Option<u32>,
    #[serde(
        default,
        alias = "cache_creation_input_tokens",
        alias = "cache_write_input_tokens"
    )]
    cache_write_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[derive(Deserialize)]
struct OaiErrorResponse {
    error: OaiErrorBody,
}

#[derive(Deserialize)]
struct OaiErrorBody {
    message: String,
}

// ---------------------------------------------------------------------------
// Model detection helpers
// ---------------------------------------------------------------------------

/// Check if the model is an OpenAI reasoning model.
fn is_reasoning_model(model: &str, provider_type: Option<&ProviderType>) -> bool {
    if let Some(provider_type) = provider_type {
        if let Some(supports_reasoning) =
            model_supports_reasoning_from_catalog(*provider_type, model)
        {
            return supports_reasoning;
        }
    }

    let m = model.to_lowercase();
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") || m.starts_with("gpt-5")
}

/// Check if the model is a DeepSeek reasoner.
fn is_deepseek_reasoner(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("deepseek-reasoner") || m.contains("deepseek-r1")
}

fn deepseek_reasoning_effort(effort: Option<&ReasoningEffort>) -> String {
    // DeepSeek accepts `high` and `max`; low/medium are compatibility aliases
    // for high, and xhigh is an alias for max.
    match effort {
        Some(ReasoningEffort::Max) | Some(ReasoningEffort::XHigh) => "max",
        _ => "high",
    }
    .to_string()
}

fn openai_reasoning_effort(effort: Option<&ReasoningEffort>) -> String {
    match effort {
        Some(ReasoningEffort::None) => "none",
        Some(ReasoningEffort::Minimal) => "minimal",
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "high",
        None => "medium",
    }
    .to_string()
}

fn requires_non_streaming_fallback(model: &str) -> bool {
    model.to_lowercase().starts_with("gpt-5.5-pro")
}

fn is_retriable_reqwest_error(error: &reqwest::Error) -> bool {
    let msg = error.to_string().to_ascii_lowercase();
    error.is_timeout()
        || error.is_connect()
        || error.is_body()
        || msg.contains("connection")
        || msg.contains("closed")
        || msg.contains("reset")
        || msg.contains("broken pipe")
        || msg.contains("incomplete")
        || msg.contains("incompleted")
        || msg.contains("unexpected eof")
        || msg.trim() == "error decoding response body"
}

async fn sleep_before_completion_retry(attempt: u32) {
    let delay_ms = 250_u64.saturating_mul(2_u64.saturating_pow(attempt.saturating_sub(1)));
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
}

fn completion_response_to_stream_chunks(
    response: CompletionResponse,
) -> Vec<Result<StreamChunk, CoreError>> {
    let mut chunks = Vec::new();

    if let Some(thinking) = response.thinking {
        if !thinking.is_empty() {
            chunks.push(Ok(StreamChunk {
                delta: String::new(),
                tool_call_delta: None,
                finish_reason: None,
                usage: None,
                thinking_delta: Some(thinking),
            }));
        }
    }

    if !response.content.is_empty() {
        chunks.push(Ok(StreamChunk {
            delta: response.content,
            tool_call_delta: None,
            finish_reason: None,
            usage: None,
            thinking_delta: None,
        }));
    }

    for (index, tool_call) in response
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        chunks.push(Ok(StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(super::ToolCallDelta {
                id: tool_call.id,
                name: Some(tool_call.name),
                arguments_delta: tool_call.arguments,
                index: Some(index as u32),
                thought_signature: tool_call.thought_signature,
            }),
            finish_reason: None,
            usage: None,
            thinking_delta: None,
        }));
    }

    chunks.push(Ok(StreamChunk {
        delta: String::new(),
        tool_call_delta: None,
        finish_reason: Some(response.finish_reason),
        usage: Some(response.usage),
        thinking_delta: None,
    }));

    chunks
}

/// Some code-specialized OpenAI-compatible models require tool-call
/// `function.arguments` to be a JSON object instead of a JSON-encoded string.
fn requires_raw_tool_arguments(model: &str, provider_type: Option<&ProviderType>) -> bool {
    if provider_type == Some(&ProviderType::Qwen) {
        return true;
    }
    let model_lower = model.to_lowercase();
    model_lower.contains("codex")
}

fn supports_anthropic_style_cache_control(
    model: &str,
    provider_type: Option<&ProviderType>,
) -> bool {
    if provider_type == Some(&ProviderType::Qwen) {
        return true;
    }
    let model_lower = model.to_lowercase();
    model_lower.contains("qwen/")
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn role_str(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn ephemeral_cache_control() -> OaiCacheControl {
    OaiCacheControl {
        r#type: "ephemeral".to_string(),
    }
}

fn add_cache_control_to_text_content(message: &mut OaiMessage) -> bool {
    let cache_control = ephemeral_cache_control();
    match &mut message.content {
        Some(OaiContent::Text(text)) => {
            let text = std::mem::take(text);
            message.content = Some(OaiContent::Parts(vec![OaiContentPart::Text {
                text,
                cache_control: Some(cache_control),
            }]));
            true
        }
        Some(OaiContent::Parts(parts)) => {
            for part in parts.iter_mut().rev() {
                if let OaiContentPart::Text {
                    cache_control: target,
                    ..
                } = part
                {
                    *target = Some(cache_control.clone());
                    return true;
                }
            }
            false
        }
        None => false,
    }
}

fn add_openai_compatible_cache_control(messages: &mut [OaiMessage]) {
    if let Some(system) = messages.iter_mut().find(|msg| msg.role == "system") {
        add_cache_control_to_text_content(system);
    }
    if let Some(last_conversation) = messages
        .iter_mut()
        .rev()
        .find(|msg| msg.role == "user" || msg.role == "assistant")
    {
        add_cache_control_to_text_content(last_conversation);
    }
}

fn parsed_tool_arguments(raw: &str) -> Option<serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value @ serde_json::Value::Object(_)) | Ok(value @ serde_json::Value::Array(_)) => {
            Some(value)
        }
        Ok(_) | Err(_) => None,
    }
}

fn normalized_tool_arguments_text(raw: &str) -> String {
    parsed_tool_arguments(raw)
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| "{}".to_string())
}

fn serialize_tool_arguments_for_history(raw: &str, raw_tool_args: bool) -> serde_json::Value {
    if raw_tool_args {
        parsed_tool_arguments(raw).unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::Value::String(normalized_tool_arguments_text(raw))
    }
}

fn convert_message(
    msg: &Message,
    include_reasoning_content: bool,
    raw_tool_args: bool,
) -> OaiMessage {
    let has_images = msg.has_images();

    // Build content: use array format when images are present, plain string otherwise.
    let content: Option<OaiContent> = if has_images {
        let parts: Vec<OaiContentPart> = msg
            .parts
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => OaiContentPart::Text {
                    text: text.clone(),
                    cache_control: None,
                },
                ContentPart::Image { media_type, data } => {
                    let url = format!("data:{media_type};base64,{data}");
                    OaiContentPart::ImageUrl {
                        image_url: OaiImageUrl { url },
                    }
                }
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(OaiContent::Parts(parts))
        }
    } else {
        let text = msg.text_content();
        if text.is_empty() {
            None
        } else {
            Some(OaiContent::Text(text))
        }
    };

    let mut oai = OaiMessage {
        role: role_str(&msg.role).to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    };

    // Assistant messages may carry tool-call requests.
    if let Some(ref calls) = msg.tool_calls {
        oai.tool_calls = Some(
            calls
                .iter()
                .map(|tc| {
                    let arguments =
                        serialize_tool_arguments_for_history(&tc.arguments, raw_tool_args);
                    OaiToolCallOut {
                        id: tc.id.clone(),
                        call_type: "function".to_string(),
                        function: OaiFunctionOut {
                            name: tc.name.clone(),
                            arguments,
                        },
                    }
                })
                .collect(),
        );
    }

    // Tool-result messages carry the originating tool_call_id.
    if msg.role == Role::Tool {
        oai.tool_call_id = msg.name.clone();
        // Tool results must be plain string content.
        oai.content = Some(OaiContent::Text(msg.text_content()));
    }

    if include_reasoning_content && msg.role == Role::Assistant {
        oai.reasoning_content = Some(
            msg.reasoning_content
                .as_deref()
                .filter(|content| !content.trim().is_empty())
                .unwrap_or(MISSING_REASONING_CONTENT_PLACEHOLDER)
                .to_string(),
        );
    }

    oai
}

fn convert_tools(tools: &[ToolDefinition], add_cache_control: bool) -> Vec<OaiTool> {
    let len = tools.len();
    tools
        .iter()
        .enumerate()
        .map(|(i, t)| OaiTool {
            tool_type: "function".to_string(),
            function: OaiToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
            cache_control: (add_cache_control && i == len - 1).then(|| OaiCacheControl {
                r#type: "ephemeral".to_string(),
            }),
        })
        .collect()
}

fn build_request_body(request: &CompletionRequest, stream: bool) -> OaiRequest {
    let is_reasoning = is_reasoning_model(&request.model, request.provider_type.as_ref());
    let is_deepseek = is_deepseek_reasoner(&request.model);
    let model_lower = request.model.to_lowercase();
    let is_deepseek_provider = matches!(request.provider_type, Some(ProviderType::DeepSeek))
        || model_lower.contains("deepseek");
    let deepseek_thinking_requested =
        request.thinking_budget.is_some() || request.reasoning_effort.is_some();
    let deepseek_thinking_mode = if is_deepseek_provider {
        Some(if deepseek_thinking_requested {
            "enabled"
        } else {
            "disabled"
        })
    } else {
        None
    };
    let deepseek_thinking_enabled = deepseek_thinking_mode == Some("enabled");
    let include_reasoning_content = is_deepseek_provider && deepseek_thinking_enabled;
    let needs_completion_tokens = is_reasoning || is_deepseek || deepseek_thinking_enabled;
    let suppress_temperature = is_reasoning || is_deepseek || deepseek_thinking_enabled;
    // Some providers/models require function arguments as JSON objects, not strings.
    let raw_tool_args = requires_raw_tool_arguments(&request.model, request.provider_type.as_ref());
    let add_cache_control =
        supports_anthropic_style_cache_control(&request.model, request.provider_type.as_ref());
    let mut messages: Vec<OaiMessage> = request
        .messages
        .iter()
        .map(|m| convert_message(m, include_reasoning_content, raw_tool_args))
        .collect();
    if add_cache_control {
        add_openai_compatible_cache_control(&mut messages);
    }

    OaiRequest {
        model: request.model.clone(),
        messages,
        temperature: if suppress_temperature {
            None
        } else {
            request.temperature
        },
        max_tokens: if needs_completion_tokens {
            None
        } else {
            request.max_tokens
        },
        max_completion_tokens: if needs_completion_tokens {
            request.max_tokens
        } else {
            None
        },
        reasoning_effort: if deepseek_thinking_enabled {
            Some(deepseek_reasoning_effort(request.reasoning_effort.as_ref()))
        } else if is_reasoning {
            request
                .reasoning_effort
                .as_ref()
                .map(|effort| openai_reasoning_effort(Some(effort)))
        } else {
            None
        },
        thinking: deepseek_thinking_mode.map(|mode| OaiThinking {
            thinking_type: mode.to_string(),
        }),
        tools: request
            .tools
            .as_ref()
            .map(|t| convert_tools(t, add_cache_control)),
        parallel_tool_calls: match request.tools.as_ref() {
            Some(tools) if !tools.is_empty() && request.parallel_tool_calls => Some(true),
            _ => None,
        },
        stop: request.stop.clone(),
        stream: if stream { Some(true) } else { None },
        stream_options: if stream {
            Some(OaiStreamOptions {
                include_usage: true,
            })
        } else {
            None
        },
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// OpenAI-compatible LLM provider.
pub struct OpenAiProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    request_timeout: Option<Duration>,
}

impl OpenAiProvider {
    /// Create a new provider with an async reqwest client.
    pub fn new(config: ProviderConfig) -> Result<Self, CoreError> {
        let timeout = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let request_timeout = configured_request_timeout(timeout);
        // SSE streams are extremely sensitive to HTTP/2 RST_STREAM frames
        // emitted by reverse proxies (e.g. Cloudflare, nginx) that terminate
        // long-lived idle h2 connections. Force HTTP/1.1 so the stream stays
        // framed at the TCP level and use a short idle-pool timeout so stale
        // keep-alive sockets are dropped before the upstream closes them.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_idle_timeout(std::time::Duration::from_secs(15))
            .pool_max_idle_per_host(5)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .http1_only()
            .build()
            .map_err(|e| CoreError::Llm(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            config,
            request_timeout,
        })
    }

    fn base_url(&self) -> &str {
        self.config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL)
    }

    fn api_key(&self) -> Result<&str, CoreError> {
        self.config
            .api_key
            .as_deref()
            .ok_or_else(|| CoreError::Llm("API key not configured".to_string()))
    }

    /// Check HTTP status and convert error responses into `CoreError`.
    async fn check_response(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, CoreError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        // 429 → rate-limited with optional Retry-After header.
        if status.as_u16() == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(CoreError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        // Try to extract the structured error message.
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<OaiErrorResponse>(&body)
            .map(|e| e.error.message)
            .unwrap_or_else(|_| format!("HTTP {status}: {body}"));

        if status.is_server_error() {
            Err(CoreError::TransientLlm(message))
        } else {
            Err(CoreError::Llm(message))
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        let url = format!("{}/models", self.base_url());
        let api_key = self.api_key()?;

        let response = with_request_timeout(
            self.client
                .get(&url)
                .header("Authorization", format!("Bearer {api_key}")),
            self.request_timeout,
        )
        .send()
        .await
        .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let models: ModelsResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse models response: {e}")))?;

        Ok(models.data.into_iter().map(|m| m.id).collect())
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key()?;
        let body = build_request_body(request, false);

        let mut attempt = 1;
        let oai: OaiResponse = loop {
            let response = match with_request_timeout(
                self.client
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("Content-Type", "application/json")
                    .json(&body),
                self.request_timeout,
            )
            .send()
            .await
            {
                Ok(response) => response,
                Err(e) if is_retriable_reqwest_error(&e) && attempt < MAX_COMPLETE_ATTEMPTS => {
                    warn!(
                        "OpenAI completion request failed on attempt {attempt}/{MAX_COMPLETE_ATTEMPTS}: {e}; retrying"
                    );
                    sleep_before_completion_retry(attempt).await;
                    attempt += 1;
                    continue;
                }
                Err(e) => return Err(CoreError::Llm(format!("Request failed: {e}"))),
            };

            let response = match self.check_response(response).await {
                Ok(response) => response,
                Err(CoreError::TransientLlm(message)) if attempt < MAX_COMPLETE_ATTEMPTS => {
                    warn!(
                        "OpenAI completion returned transient error on attempt {attempt}/{MAX_COMPLETE_ATTEMPTS}: {message}; retrying"
                    );
                    sleep_before_completion_retry(attempt).await;
                    attempt += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };

            match response.json().await {
                Ok(oai) => break oai,
                Err(e) if is_retriable_reqwest_error(&e) && attempt < MAX_COMPLETE_ATTEMPTS => {
                    warn!(
                        "OpenAI completion response body failed on attempt {attempt}/{MAX_COMPLETE_ATTEMPTS}: {e}; retrying"
                    );
                    sleep_before_completion_retry(attempt).await;
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    let message = format!("Failed to parse completion response: {e}");
                    return if is_retriable_reqwest_error(&e) {
                        Err(CoreError::TransientLlm(message))
                    } else {
                        Err(CoreError::Llm(message))
                    };
                }
            }
        };

        let choice = oai
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| CoreError::Llm("No choices in response".to_string()))?;

        let tool_calls = choice.message.tool_calls.map(|tcs| {
            tcs.into_iter()
                .map(|tc| ToolCallRequest {
                    id: tc.id,
                    name: tc.function.name,
                    arguments: tc.function.arguments.into_argument_string(),
                    thought_signature: None,
                })
                .collect()
        });

        let finish_reason = choice
            .finish_reason
            .as_deref()
            .map(parse_finish_reason)
            .unwrap_or(FinishReason::Other);

        let usage = oai
            .usage
            .map(|u| {
                let prompt_details = u.prompt_tokens_details;
                Usage {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    total_tokens: u.total_tokens,
                    thinking_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
                    cache_read_tokens: prompt_details.as_ref().and_then(|d| d.cached_tokens),
                    cache_creation_tokens: prompt_details.and_then(|d| d.cache_write_tokens),
                }
            })
            .unwrap_or_default();

        Ok(CompletionResponse {
            content: choice.message.content.unwrap_or_default(),
            tool_calls,
            finish_reason,
            usage,
            thinking: choice.message.reasoning_content,
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        if requires_non_streaming_fallback(&request.model) {
            let response = self.complete(request).await?;
            return Ok(Box::pin(futures::stream::iter(
                completion_response_to_stream_chunks(response),
            )));
        }

        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key()?;
        let body = build_request_body(request, true);

        info!("OpenAI stream request to {url}, model={}", request.model);
        let body_json = serde_json::to_string(&body).unwrap_or_default();
        debug!("Request body: {} bytes", body_json.len());

        let response = send_stream_start_request(
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body),
            self.request_timeout,
            "OpenAI stream request",
        )
        .await
        .map_err(|e| {
            error!("Stream send failed: {e}");
            e
        })?;

        info!("Stream response status: {}", response.status());
        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);
        info!("SSE stream started");

        tokio::spawn(async move {
            if let Err(e) = parse_sse_stream(response, tx.clone()).await {
                error!("SSE stream error: {e}");
                let _ = tx.send(Err(e)).await;
            }
            info!("SSE stream ended");
        });

        let stream = futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        self.list_models().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_delayed_sse_response(listener: tokio::net::TcpListener) -> std::io::Result<()> {
        let (mut socket, _) = listener.accept().await?;
        let mut request = Vec::new();
        let mut headers_end = None;

        while headers_end.is_none() {
            let mut buf = [0u8; 1024];
            let n = socket.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            request.extend_from_slice(&buf[..n]);
            headers_end = request.windows(4).position(|window| window == b"\r\n\r\n");
        }

        let headers_end = headers_end.expect("headers end") + 4;
        let headers = String::from_utf8_lossy(&request[..headers_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while request.len() < headers_end + content_length {
            let mut buf = [0u8; 1024];
            let n = socket.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }

        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await?;
        socket
            .write_all(
                br#"data: {"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}

"#,
            )
            .await?;
        socket.flush().await?;

        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

        socket
            .write_all(
                br#"data: {"choices":[{"delta":{"content":" world"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}

data: [DONE]

"#,
            )
            .await?;
        socket.flush().await?;
        socket.shutdown().await
    }

    #[test]
    fn deepseek_v4_thinking_request_uses_supported_wire_shape() {
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: Some(1024),
            reasoning_effort: None,
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_completion_tokens"], 100);
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[tokio::test]
    async fn stream_timeout_does_not_cap_total_response_body_duration() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(serve_delayed_sse_response(listener));

        let provider = OpenAiProvider::new(ProviderConfig {
            provider_type: ProviderType::OpenAi,
            base_url: Some(format!("http://{addr}/v1")),
            api_key: Some("test-key".to_string()),
            org_id: None,
            timeout_secs: Some(1),
        })
        .expect("provider");

        let request = CompletionRequest {
            model: "test-model".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: None,
            max_tokens: Some(32),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenAi),
            parallel_tool_calls: true,
        };

        let mut stream = provider.stream(&request).await.expect("start stream");
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.expect("stream chunk");
            text.push_str(&chunk.delta);
        }

        server.await.expect("server task").expect("server result");
        assert_eq!(text, "hello world");
    }

    #[test]
    fn deepseek_v4_max_reasoning_uses_max_effort() {
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: Some(1024),
            reasoning_effort: Some(ReasoningEffort::Max),
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "max");
    }

    #[test]
    fn deepseek_reasoning_effort_enables_thinking_without_budget() {
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_completion_tokens"], 100);
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn deepseek_thinking_history_replays_reasoning_content() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![ContentPart::Text {
                text: "answer".to_string(),
            }],
            name: None,
            tool_calls: None,
            reasoning_content: Some("prior reasoning".to_string()),
        };
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello"), assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["messages"][1]["reasoning_content"], "prior reasoning");
    }

    #[test]
    fn deepseek_thinking_history_replays_reasoning_content_with_tool_calls() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "run_shell".to_string(),
                arguments: "{\"program\":\"python\",\"args\":[\"-c\",\"print(1)\"]}".to_string(),
                thought_signature: None,
            }]),
            reasoning_content: Some("Need to check whether python-docx is installed.".to_string()),
        };
        let mut tool = Message::text(Role::Tool, "python-docx 1.2.0");
        tool.name = Some("call_1".to_string());
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "make a docx"), assistant, tool],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(
            body["messages"][1]["reasoning_content"],
            "Need to check whether python-docx is installed."
        );
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
    }

    #[test]
    fn deepseek_thinking_history_replays_fallback_reasoning_for_legacy_assistant() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_legacy".to_string(),
                name: "run_shell".to_string(),
                arguments: "{\"program\":\"python\",\"args\":[\"-c\",\"print(1)\"]}".to_string(),
                thought_signature: None,
            }]),
            reasoning_content: None,
        };
        let mut tool = Message::text(Role::Tool, "ok");
        tool.name = Some("call_legacy".to_string());
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "make a docx"), assistant, tool],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(
            body["messages"][1]["reasoning_content"],
            "[reasoning content unavailable in local history]"
        );
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call_legacy");
    }

    #[test]
    fn deepseek_disabled_thinking_omits_reasoning_content() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![ContentPart::Text {
                text: "answer".to_string(),
            }],
            name: None,
            tool_calls: None,
            reasoning_content: Some("prior reasoning".to_string()),
        };
        let request = CompletionRequest {
            model: "deepseek-v4-chat".to_string(),
            messages: vec![Message::text(Role::User, "hello"), assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::DeepSeek),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn openai_reasoning_effort_is_only_sent_when_configured() {
        let request = CompletionRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenAi),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert!(body.get("reasoning_effort").is_none());

        let request = CompletionRequest {
            reasoning_effort: Some(ReasoningEffort::None),
            ..request
        };
        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert_eq!(body["reasoning_effort"], "none");
    }

    #[test]
    fn openai_reasoning_detection_prefers_catalog_then_legacy_fallback() {
        let request = CompletionRequest {
            model: "gpt-5.5-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::OpenAi),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("temperature").is_some());

        let request = CompletionRequest {
            model: "gpt-5-future".to_string(),
            ..request
        };
        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_completion_tokens"], 100);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn qwen_history_tool_arguments_are_sent_as_json_objects() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "run_shell".to_string(),
                arguments: "{\"program\":\"python\",\"args\":[\"-c\",\"print(1)\"]}".to_string(),
                thought_signature: None,
            }]),
            reasoning_content: None,
        };
        let request = CompletionRequest {
            model: "qwen3-coder".to_string(),
            messages: vec![assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            serde_json::json!({
                "program": "python",
                "args": ["-c", "print(1)"]
            })
        );
    }

    #[test]
    fn qwen_requests_add_explicit_prompt_cache_markers() {
        let request = CompletionRequest {
            model: "qwen3.7-max".to_string(),
            messages: vec![
                Message::text(Role::System, "stable system"),
                Message::text(Role::System, "runtime plan"),
                Message::text(Role::User, "hello"),
            ],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: Some(vec![ToolDefinition {
                name: "search".into(),
                description: "Search".into(),
                parameters: serde_json::json!({"type":"object"}),
            }]),
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert!(body["messages"][1]["content"].is_string());
        assert_eq!(
            body["messages"][2]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert_eq!(
            body["tools"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn openai_usage_deserializes_prompt_cache_tokens() {
        let response: OaiResponse = serde_json::from_value(serde_json::json!({
            "choices": [{
                "message": {"content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": {
                    "cached_tokens": 64,
                    "cache_write_tokens": 32
                }
            }
        }))
        .unwrap();

        let details = response.usage.unwrap().prompt_tokens_details.unwrap();
        assert_eq!(details.cached_tokens, Some(64));
        assert_eq!(details.cache_write_tokens, Some(32));
    }

    #[test]
    fn invalid_history_tool_arguments_are_replaced_before_replay() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_bad".to_string(),
                name: "run_shell".to_string(),
                arguments: "{not valid json".to_string(),
                thought_signature: None,
            }]),
            reasoning_content: None,
        };
        let request = CompletionRequest {
            model: "qwen3-coder".to_string(),
            messages: vec![assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            serde_json::json!({})
        );

        let request = CompletionRequest {
            provider_type: Some(ProviderType::OpenAi),
            model: "gpt-5.5".to_string(),
            ..request
        };
        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
    }

    #[test]
    fn response_tool_arguments_accept_json_object_wire_shape() {
        let response: OaiResponse = serde_json::from_value(serde_json::json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "lookup",
                            "arguments": { "q": "x" }
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": null
        }))
        .unwrap();

        let tool_call = response
            .choices
            .into_iter()
            .next()
            .unwrap()
            .message
            .tool_calls
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(
            tool_call.function.arguments.into_argument_string(),
            "{\"q\":\"x\"}"
        );
    }

    #[test]
    fn completion_response_stream_fallback_preserves_content_tool_calls_and_usage() {
        let chunks = completion_response_to_stream_chunks(CompletionResponse {
            content: "done".to_string(),
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: "{\"q\":\"x\"}".to_string(),
                thought_signature: None,
            }]),
            finish_reason: FinishReason::ToolCalls,
            usage: Usage {
                prompt_tokens: 3,
                completion_tokens: 4,
                total_tokens: 7,
                thinking_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
            },
            thinking: Some("thinking".to_string()),
        });

        assert_eq!(chunks.len(), 4);
        assert_eq!(
            chunks[0].as_ref().unwrap().thinking_delta.as_deref(),
            Some("thinking")
        );
        assert_eq!(chunks[1].as_ref().unwrap().delta, "done");
        let tool_delta = chunks[2]
            .as_ref()
            .unwrap()
            .tool_call_delta
            .as_ref()
            .expect("tool delta should be emitted");
        assert_eq!(tool_delta.id, "call_1");
        assert_eq!(tool_delta.name.as_deref(), Some("lookup"));
        assert_eq!(tool_delta.arguments_delta, "{\"q\":\"x\"}");
        assert_eq!(
            chunks[3].as_ref().unwrap().finish_reason,
            Some(FinishReason::ToolCalls)
        );
        assert_eq!(
            chunks[3]
                .as_ref()
                .unwrap()
                .usage
                .as_ref()
                .unwrap()
                .total_tokens,
            7
        );
    }
}
