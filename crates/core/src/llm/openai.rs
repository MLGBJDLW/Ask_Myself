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

use super::prompt_cache::{resolve_prompt_cache_profile, PromptCacheApiStyle, PromptCacheProfile};
use super::reasoning_profile::{
    resolve_reasoning_profile, ReasoningApiStyle, ReasoningBudgetField, ReasoningEffortField,
    ReasoningHistoryEncoding, ThinkingModeControl,
};
use super::transport::{shared_http_transport, HttpTransport};
use super::{
    configured_request_timeout, send_stream_start_request, serialized_json_body,
    streaming::parse_sse_stream, with_request_timeout, CompletionRequest, CompletionResponse,
    ContentPart, FinishReason, LlmProvider, Message, ProviderConfig, ProviderType, Role,
    StreamChunk, ToolCallRequest, ToolDefinition, Usage,
};
#[cfg(test)]
use super::{CacheBoundaryHint, PromptStability, ReasoningEffort};
use crate::error::CoreError;
use crate::provider_catalog::model_supports_reasoning_from_catalog;
use std::sync::Arc;

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
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OaiReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<OaiThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserve_thinking: Option<bool>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    keep: Option<String>,
}

#[derive(Serialize)]
struct OaiReasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
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
    Thinking {
        thinking: Vec<OaiThinkingContentPart>,
    },
}

#[derive(Serialize)]
struct OaiThinkingContentPart {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
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
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    openrouter_metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OaiResponseMessage {
    content: Option<serde_json::Value>,
    tool_calls: Option<Vec<OaiToolCallIn>>,
    #[serde(default, alias = "reasoningContent")]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<serde_json::Value>,
    #[serde(default, alias = "reasoningDetails")]
    reasoning_details: Option<serde_json::Value>,
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

#[derive(Deserialize, Serialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u32>,
    #[serde(default)]
    prompt_cache_miss_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens_details: Option<OaiCompletionTokensDetails>,
    #[serde(default)]
    prompt_tokens_details: Option<OaiPromptTokensDetails>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
struct OaiCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum OaiCacheCreationUsage {
    Tokens(u32),
    Details(OaiCacheCreationDetails),
}

impl OaiCacheCreationUsage {
    fn input_tokens(&self) -> Option<u32> {
        match self {
            Self::Tokens(tokens) => Some(*tokens),
            Self::Details(details) => details.cache_creation_input_tokens,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct OaiCacheCreationDetails {
    #[serde(
        default,
        alias = "cache_write_input_tokens",
        alias = "cache_creation_tokens",
        alias = "ephemeral_5m_input_tokens"
    )]
    cache_creation_input_tokens: Option<u32>,
}

#[derive(Deserialize, Serialize)]
struct OaiPromptTokensDetails {
    #[serde(default, alias = "cache_read_input_tokens", alias = "cachedTokens")]
    cached_tokens: Option<u32>,
    #[serde(
        default,
        alias = "cache_creation",
        alias = "cache_creation_input_tokens",
        alias = "cache_write_input_tokens",
        alias = "cache_write_tokens"
    )]
    cache_creation: Option<OaiCacheCreationUsage>,
}

impl OaiPromptTokensDetails {
    fn cache_creation_tokens(&self) -> Option<u32> {
        self.cache_creation
            .as_ref()
            .and_then(OaiCacheCreationUsage::input_tokens)
    }
}

#[cfg(test)]
fn usage_from_oai_usage(u: OaiUsage) -> Usage {
    usage_from_oai_usage_with_route(u, None, None, None)
}

fn usage_from_oai_usage_with_route(
    u: OaiUsage,
    generation_id: Option<String>,
    response_model: Option<String>,
    openrouter_metadata: Option<serde_json::Value>,
) -> Usage {
    let provider_usage = serde_json::to_value(&u).unwrap_or(serde_json::Value::Null);
    let prompt_details = u.prompt_tokens_details;
    let cache_read_tokens = super::prompt_cache::openai_compatible_cache_read_tokens(
        prompt_details.as_ref().and_then(|d| d.cached_tokens),
        u.prompt_cache_hit_tokens,
    );
    Usage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
        thinking_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
        tool_prompt_tokens: None,
        cache_read_tokens,
        cache_miss_tokens: u.prompt_cache_miss_tokens,
        cache_creation_tokens: prompt_details
            .as_ref()
            .and_then(OaiPromptTokensDetails::cache_creation_tokens),
        provider_raw: Some(serde_json::json!({
            "usage": provider_usage,
            "generationId": generation_id,
            "responseModel": response_model,
            "openrouterMetadata": openrouter_metadata,
        })),
    }
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

fn reasoning_value_to_text(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Array(parts) => {
            let text = parts
                .into_iter()
                .filter_map(reasoning_value_to_text)
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        serde_json::Value::Object(mut object) => object
            .remove("text")
            .or_else(|| object.remove("content"))
            .or_else(|| object.remove("summary_text"))
            .or_else(|| object.remove("summaryText"))
            .or_else(|| object.remove("reasoning_details"))
            .or_else(|| object.remove("reasoningDetails"))
            .and_then(reasoning_value_to_text),
        _ => None,
    }
}

fn is_openrouter_config(config: &ProviderConfig) -> bool {
    matches!(config.provider_type, ProviderType::OpenRouter)
        || config
            .base_url
            .as_deref()
            .and_then(|url| reqwest::Url::parse(url).ok())
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .as_deref()
            == Some("openrouter.ai")
}

fn apply_openrouter_headers(
    builder: reqwest::RequestBuilder,
    config: &ProviderConfig,
) -> reqwest::RequestBuilder {
    if is_openrouter_config(config) {
        builder
            .header("X-OpenRouter-Title", "Nexa")
            .header("X-OpenRouter-Metadata", "enabled")
    } else {
        builder
    }
}

pub(crate) fn requires_non_streaming_fallback(model: &str) -> bool {
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

fn is_alibaba_hosted_qwen(model: &str, provider_type: Option<&ProviderType>) -> bool {
    if provider_type != Some(&ProviderType::AlibabaModelStudio) {
        return false;
    }
    let model_lower = model.to_ascii_lowercase();
    model_lower.starts_with("qwen") || model_lower.starts_with("qwq")
}

/// Some code-specialized OpenAI-compatible models require tool-call
/// `function.arguments` to be a JSON object instead of a JSON-encoded string.
fn requires_raw_tool_arguments(model: &str, provider_type: Option<&ProviderType>) -> bool {
    if provider_type == Some(&ProviderType::Qwen) || is_alibaba_hosted_qwen(model, provider_type) {
        return true;
    }
    let model_lower = model.to_lowercase();
    model_lower.contains("codex")
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

fn is_leading_system_message(messages: &[Message], index: usize) -> bool {
    messages
        .get(index)
        .is_some_and(|message| message.role == Role::System)
        && messages
            .iter()
            .take(index)
            .all(|message| message.role == Role::System)
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

fn add_profile_cache_control_for_request(
    request: &CompletionRequest,
    messages: &mut [OaiMessage],
    profile: &PromptCacheProfile,
) {
    if !profile.uses_message_breakpoints()
        || !profile.request_is_eligible(&request.messages, request.tools.as_deref())
    {
        return;
    }

    let limit = usize::from(profile.max_breakpoints.unwrap_or(0));
    let mut latest_by_boundary = std::collections::BTreeMap::new();
    for (index, message) in request.messages.iter().enumerate() {
        if let Some((stability, boundary)) = message.prompt_cache_hint() {
            if stability != super::PromptStability::Volatile {
                latest_by_boundary.insert(boundary, index);
            }
        }
    }
    let mut candidates = latest_by_boundary.into_values().collect::<Vec<_>>();
    candidates.sort_unstable();
    for index in candidates.into_iter().rev().take(limit).rev() {
        if let Some(message) = messages.get_mut(index) {
            add_cache_control_to_text_content(message);
        }
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
    wire_role: &str,
    include_reasoning_content: bool,
    reasoning_history_encoding: ReasoningHistoryEncoding,
    synthesize_missing_reasoning_content: bool,
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
        role: wire_role.to_string(),
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
        let reasoning = msg
            .reasoning_content
            .as_deref()
            .filter(|content| !content.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                synthesize_missing_reasoning_content
                    .then(|| MISSING_REASONING_CONTENT_PLACEHOLDER.to_string())
            });
        if let Some(reasoning) = reasoning {
            match reasoning_history_encoding {
                ReasoningHistoryEncoding::ReasoningContent => {
                    oai.reasoning_content = Some(reasoning);
                }
                ReasoningHistoryEncoding::ThinkTags => {
                    let answer = msg.text_content();
                    oai.content = Some(OaiContent::Text(format!(
                        "<think>\n{reasoning}\n</think>\n{answer}"
                    )));
                }
                ReasoningHistoryEncoding::MistralContentChunks => {
                    let mut parts = vec![OaiContentPart::Thinking {
                        thinking: vec![OaiThinkingContentPart {
                            content_type: "text".to_string(),
                            text: reasoning,
                        }],
                    }];
                    let answer = msg.text_content();
                    if !answer.is_empty() {
                        parts.push(OaiContentPart::Text {
                            text: answer,
                            cache_control: None,
                        });
                    }
                    oai.content = Some(OaiContent::Parts(parts));
                }
            }
        }
    }

    oai
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<OaiTool> {
    tools
        .iter()
        .map(|t| OaiTool {
            tool_type: "function".to_string(),
            function: OaiToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
            cache_control: None,
        })
        .collect()
}

#[cfg(test)]
fn build_request_body(request: &CompletionRequest, stream: bool) -> OaiRequest {
    build_request_body_with_config(request, stream, None)
}

fn build_request_body_with_config(
    request: &CompletionRequest,
    stream: bool,
    config: Option<&ProviderConfig>,
) -> OaiRequest {
    let provider_type = request
        .provider_type
        .or_else(|| config.map(|config| config.provider_type))
        .unwrap_or(ProviderType::Custom);
    let reasoning_profile = resolve_reasoning_profile(
        provider_type,
        config.and_then(|config| config.base_url.as_deref()),
        ReasoningApiStyle::OpenAiChatCompletions,
        &request.model,
    );
    let reasoning_supported = reasoning_profile.id != "openai-reasoning-v1"
        || is_reasoning_model(&request.model, Some(&provider_type));
    let requested_reasoning_mode = reasoning_profile.requested_mode(
        request.reasoning_enabled,
        request.reasoning_effort.as_ref(),
        request.thinking_budget,
    );
    let effort_can_encode_disabled =
        reasoning_profile.mode_control == ThinkingModeControl::ProviderDefault;
    let requested_effort = request.reasoning_effort.as_ref().or_else(|| {
        (requested_reasoning_mode == Some(true) && request.thinking_budget.is_none())
            .then_some(reasoning_profile.default_effort.as_ref())
            .flatten()
    });
    let wire_effort = (reasoning_supported
        && (requested_reasoning_mode != Some(false) || effort_can_encode_disabled))
        .then(|| reasoning_profile.wire_effort(requested_effort))
        .flatten();
    let wire_budget = (reasoning_supported && requested_reasoning_mode != Some(false))
        .then(|| reasoning_profile.wire_budget(request.thinking_budget, wire_effort.is_some()))
        .flatten();
    let include_reasoning_content =
        reasoning_supported && reasoning_profile.should_replay_reasoning(requested_reasoning_mode);
    let reasoning_history_encoding = reasoning_profile.reasoning_history_encoding;
    let synthesize_missing_reasoning_content =
        include_reasoning_content && reasoning_profile.synthesize_missing_reasoning_history;
    let needs_completion_tokens = reasoning_profile.use_max_completion_tokens
        && is_reasoning_model(&request.model, Some(&provider_type));
    let suppress_temperature = reasoning_supported
        && reasoning_profile.omit_temperature_when_reasoning
        && requested_reasoning_mode != Some(false);
    // Some providers/models require function arguments as JSON objects, not strings.
    let raw_tool_args = requires_raw_tool_arguments(&request.model, request.provider_type.as_ref());
    let cache_profile = resolve_prompt_cache_profile(
        provider_type,
        config.and_then(|config| config.base_url.as_deref()),
        PromptCacheApiStyle::OpenAiCompatible,
        &request.model,
    );
    let mut messages: Vec<OaiMessage> = request
        .messages
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let wire_role =
                if m.role == Role::System && !is_leading_system_message(&request.messages, index) {
                    "user"
                } else {
                    role_str(&m.role)
                };
            convert_message(
                m,
                wire_role,
                include_reasoning_content,
                reasoning_history_encoding,
                synthesize_missing_reasoning_content,
                raw_tool_args,
            )
        })
        .collect();
    add_profile_cache_control_for_request(request, &mut messages, &cache_profile);

    OaiRequest {
        model: request.model.clone(),
        messages,
        session_id: (cache_profile.routing_session_affinity)
            .then(|| request.routing_session_id.clone())
            .flatten(),
        prompt_cache_key: super::prompt_cache::openai_prompt_cache_key(
            &cache_profile,
            &request.model,
            &request.messages,
            request.tools.as_deref(),
        ),
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
        reasoning_effort: (reasoning_profile.effort_field == ReasoningEffortField::TopLevel)
            .then_some(wire_effort.clone())
            .flatten(),
        reasoning: if reasoning_profile.effort_field == ReasoningEffortField::NestedReasoning
            || reasoning_profile.budget_field == ReasoningBudgetField::NestedReasoning
        {
            (wire_effort.is_some() || wire_budget.is_some()).then(|| OaiReasoning {
                effort: wire_effort.clone(),
                max_tokens: wire_budget,
            })
        } else {
            None
        },
        thinking: match reasoning_profile.mode_control {
            ThinkingModeControl::ThinkingType => {
                requested_reasoning_mode.map(|enabled| OaiThinking {
                    thinking_type: if enabled { "enabled" } else { "disabled" }.to_string(),
                    keep: None,
                })
            }
            ThinkingModeControl::ThinkingTypeWithKeep => {
                requested_reasoning_mode.map(|enabled| OaiThinking {
                    thinking_type: if enabled { "enabled" } else { "disabled" }.to_string(),
                    keep: enabled.then(|| "all".to_string()),
                })
            }
            ThinkingModeControl::AdaptiveThinking => {
                requested_reasoning_mode.map(|enabled| OaiThinking {
                    thinking_type: if enabled { "adaptive" } else { "disabled" }.to_string(),
                    keep: None,
                })
            }
            _ => None,
        },
        enable_thinking: (reasoning_profile.mode_control == ThinkingModeControl::EnableThinking)
            .then_some(requested_reasoning_mode)
            .flatten(),
        thinking_budget: (reasoning_profile.budget_field == ReasoningBudgetField::ThinkingBudget)
            .then_some(wire_budget)
            .flatten(),
        preserve_thinking: (reasoning_profile.send_preserve_thinking
            && requested_reasoning_mode != Some(false))
        .then_some(true),
        tools: request.tools.as_ref().map(|t| convert_tools(t)),
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

fn hosted_search_context(
    request: &CompletionRequest,
) -> Option<(
    super::native_search::NativeSearchDialect,
    super::native_search::SearchExecutionMode,
    crate::model_catalog::NativeWebSearchCapability,
)> {
    let tools = request.tools.as_deref()?;
    let dialect = super::native_search::marker_dialect(tools)?;
    if !matches!(
        dialect,
        super::native_search::NativeSearchDialect::OpenAiResponses
            | super::native_search::NativeSearchDialect::DeepSeekResponses
    ) {
        return None;
    }
    Some((
        dialect,
        super::native_search::marker_mode(tools)?,
        super::native_search::marker_capability(tools)?,
    ))
}

fn hosted_search_requires_client_tools(
    request: &CompletionRequest,
    mode: super::native_search::SearchExecutionMode,
) -> bool {
    request
        .tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|tool| !super::native_search::is_native_marker(tool))
        .any(|tool| {
            tool.name != super::native_search::LOCAL_WEB_SEARCH_TOOL
                || mode == super::native_search::SearchExecutionMode::Hybrid
        })
}

fn without_native_search_marker(request: &CompletionRequest) -> CompletionRequest {
    CompletionRequest {
        tools: request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .filter(|tool| !super::native_search::is_native_marker(tool))
                .cloned()
                .collect()
        }),
        ..request.clone()
    }
}

const RESPONSES_REASONING_SIGNATURE_PREFIX: &str = "nexa.responses.reasoning.v1:";

fn replayable_responses_reasoning(tool_calls: &[ToolCallRequest]) -> Vec<serde_json::Value> {
    tool_calls
        .iter()
        .filter_map(|tool_call| tool_call.thought_signature.as_deref())
        .find_map(|signature| {
            signature
                .strip_prefix(RESPONSES_REASONING_SIGNATURE_PREFIX)
                .and_then(|payload| serde_json::from_str(payload).ok())
        })
        .unwrap_or_default()
}

fn responses_input_items(messages: &[Message]) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    for message in messages {
        if message.role == Role::Tool {
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": message.name,
                "output": message.text_content(),
            }));
            continue;
        }

        if message.role == Role::Assistant {
            items.extend(replayable_responses_reasoning(
                message.tool_calls.as_deref().unwrap_or_default(),
            ));
        }

        let mut content = Vec::new();
        for part in &message.parts {
            match part {
                ContentPart::Text { text } => content.push(serde_json::json!({
                    "type": if message.role == Role::Assistant { "output_text" } else { "input_text" },
                    "text": text,
                })),
                ContentPart::Image { media_type, data } => content.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("data:{media_type};base64,{data}"),
                })),
            }
        }
        if !content.is_empty() {
            items.push(serde_json::json!({
                "type": "message",
                "role": role_str(&message.role),
                "content": content,
            }));
        }
        for tool_call in message.tool_calls.as_deref().unwrap_or_default() {
            items.push(serde_json::json!({
                "type": "function_call",
                "call_id": tool_call.id,
                "name": tool_call.name,
                "arguments": tool_call.arguments,
            }));
        }
    }
    items
}

fn responses_tools(
    request: &CompletionRequest,
    dialect: super::native_search::NativeSearchDialect,
    mode: super::native_search::SearchExecutionMode,
    capability: crate::model_catalog::NativeWebSearchCapability,
) -> Vec<serde_json::Value> {
    let mut tools = vec![super::native_search::compile_hosted_search_tool(
        dialect,
        capability,
        &super::native_search::WebSearchIntent::default(),
    )];
    let include_local_search = mode == super::native_search::SearchExecutionMode::Hybrid;
    tools.extend(
        request
            .tools
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|tool| !super::native_search::is_native_marker(tool))
            .filter(|_| capability.can_mix_client_tools)
            .filter(|tool| {
                include_local_search || tool.name != super::native_search::LOCAL_WEB_SEARCH_TOOL
            })
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            }),
    );
    tools
}

fn build_responses_request(
    request: &CompletionRequest,
    dialect: super::native_search::NativeSearchDialect,
    mode: super::native_search::SearchExecutionMode,
    capability: crate::model_catalog::NativeWebSearchCapability,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        "input": responses_input_items(&request.messages),
        "tools": responses_tools(request, dialect, mode, capability),
        "parallel_tool_calls": request.parallel_tool_calls,
        "store": false,
    });
    if let Some(max_tokens) = request.max_tokens {
        body["max_output_tokens"] = serde_json::json!(max_tokens);
    }
    if dialect == super::native_search::NativeSearchDialect::OpenAiResponses {
        if capability.can_mix_client_tools {
            // Stateless Responses tool loops must replay encrypted reasoning
            // items alongside function calls. The returned payload is kept in
            // ToolCallRequest::thought_signature and never shown as reasoning.
            body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
        }
        if let Some(effort) = request.reasoning_effort.as_ref() {
            body["reasoning"] = serde_json::json!({ "effort": effort.to_string() });
        } else if let Some(temperature) = request.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }
    }
    body
}

fn parse_responses_completion(
    value: serde_json::Value,
    dialect: super::native_search::NativeSearchDialect,
    capability: crate::model_catalog::NativeWebSearchCapability,
) -> Result<CompletionResponse, CoreError> {
    let output = value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| CoreError::Llm("Responses payload did not contain output items".into()))?;
    let mut content = String::new();
    let mut thinking = Vec::new();
    let mut tool_calls = Vec::new();
    let mut query = None;
    let mut citations = Vec::new();
    let mut reasoning_replay = Vec::new();

    for item in output {
        match item.get("type").and_then(serde_json::Value::as_str) {
            Some("web_search_call") => {
                query = item
                    .pointer("/action/query")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        item.pointer("/action/queries")
                            .and_then(serde_json::Value::as_array)
                            .map(|queries| {
                                queries
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(" | ")
                            })
                            .filter(|queries| !queries.is_empty())
                    });
            }
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if part.get("type").and_then(serde_json::Value::as_str) == Some("output_text") {
                        if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                            content.push_str(text);
                        }
                        if capability.supports_citations {
                            for annotation in part
                                .get("annotations")
                                .and_then(serde_json::Value::as_array)
                                .into_iter()
                                .flatten()
                            {
                                if annotation.get("type").and_then(serde_json::Value::as_str)
                                    != Some("url_citation")
                                {
                                    continue;
                                }
                                if let Some(url) =
                                    annotation.get("url").and_then(serde_json::Value::as_str)
                                {
                                    citations.push(super::native_search::SearchCitation {
                                        url: url.to_string(),
                                        title: annotation
                                            .get("title")
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_string),
                                        start_index: annotation
                                            .get("start_index")
                                            .and_then(serde_json::Value::as_u64)
                                            .and_then(|value| u32::try_from(value).ok()),
                                        end_index: annotation
                                            .get("end_index")
                                            .and_then(serde_json::Value::as_u64)
                                            .and_then(|value| u32::try_from(value).ok()),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            Some("function_call") => tool_calls.push(ToolCallRequest {
                id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: item
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments: item
                    .get("arguments")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("{}")
                    .to_string(),
                thought_signature: None,
            }),
            Some("reasoning") => {
                if item
                    .get("encrypted_content")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.is_empty())
                {
                    reasoning_replay.push(item.clone());
                }
                for summary in item
                    .get("summary")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(text) = summary.get("text").and_then(serde_json::Value::as_str) {
                        thinking.push(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    if !reasoning_replay.is_empty() && !tool_calls.is_empty() {
        tool_calls[0].thought_signature = Some(format!(
            "{RESPONSES_REASONING_SIGNATURE_PREFIX}{}",
            serde_json::to_string(&reasoning_replay)?
        ));
    }

    if capability.supports_citations && !citations.is_empty() {
        content.push_str(&super::native_search::render_citation_appendix(
            &super::native_search::SearchEvidence {
                dialect,
                query,
                citations,
            },
        ));
    }
    let usage_value = value.get("usage").cloned().unwrap_or_default();
    let provider_raw = usage_value.as_object().map(|_| usage_value.clone());
    let prompt_tokens = usage_value
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let completion_tokens = usage_value
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("completed");
    let finish_reason = if !tool_calls.is_empty() {
        FinishReason::ToolCalls
    } else if status == "incomplete"
        && value
            .pointer("/incomplete_details/reason")
            .and_then(serde_json::Value::as_str)
            == Some("max_output_tokens")
    {
        FinishReason::Length
    } else if status == "completed" {
        FinishReason::Stop
    } else {
        FinishReason::Other
    };
    Ok(CompletionResponse {
        content,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        finish_reason,
        usage: Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: usage_value
                .get("total_tokens")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens)),
            provider_raw,
            ..Usage::default()
        },
        thinking: (!thinking.is_empty()).then(|| thinking.join("\n")),
    })
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// OpenAI-compatible LLM provider.
pub struct OpenAiProvider {
    transport: Arc<HttpTransport>,
    config: ProviderConfig,
    request_timeout: Option<Duration>,
}

impl OpenAiProvider {
    /// Create a new provider with an async reqwest client.
    pub fn new(config: ProviderConfig) -> Result<Self, CoreError> {
        let timeout = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let request_timeout = configured_request_timeout(timeout);
        let transport = shared_http_transport(&config)?;

        Ok(Self {
            transport,
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

    async fn complete_hosted_search(
        &self,
        request: &CompletionRequest,
        dialect: super::native_search::NativeSearchDialect,
        mode: super::native_search::SearchExecutionMode,
        capability: crate::model_catalog::NativeWebSearchCapability,
    ) -> Result<CompletionResponse, CoreError> {
        let url = format!("{}/responses", self.base_url().trim_end_matches('/'));
        let body = build_responses_request(request, dialect, mode, capability);
        let body_bytes = serialized_json_body(&body, "Responses hosted-search request")?;
        let api_key = self.api_key()?;
        info!(
            "Responses hosted-search request to {url}, model={}, dialect={dialect:?}",
            request.model
        );
        let response = with_request_timeout(
            apply_openrouter_headers(
                self.transport
                    .client()
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("Content-Type", "application/json")
                    .body(body_bytes),
                &self.config,
            ),
            self.request_timeout,
        )
        .send()
        .await
        .inspect_err(|error| self.transport.record_transport_failure(&error.to_string()))
        .map_err(|error| CoreError::Llm(format!("Responses request failed: {error}")))?;
        let response = self.check_response(response).await?;
        let value = response
            .json::<serde_json::Value>()
            .await
            .inspect_err(|error| self.transport.record_transport_failure(&error.to_string()))
            .map_err(|error| {
                CoreError::Llm(format!("Failed to parse Responses payload: {error}"))
            })?;
        let parsed = parse_responses_completion(value, dialect, capability)?;
        self.transport.record_transport_success();
        Ok(parsed)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn prompt_cache_profile(&self, model: &str) -> PromptCacheProfile {
        resolve_prompt_cache_profile(
            self.config.provider_type,
            self.config.base_url.as_deref(),
            PromptCacheApiStyle::OpenAiCompatible,
            model,
        )
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        let url = format!("{}/models", self.base_url());
        let api_key = self.api_key()?;

        let response = with_request_timeout(
            apply_openrouter_headers(
                self.transport
                    .client()
                    .get(&url)
                    .header("Authorization", format!("Bearer {api_key}")),
                &self.config,
            ),
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
        let fallback_request;
        let request = if let Some((dialect, mode, capability)) = hosted_search_context(request) {
            if !capability.can_mix_client_tools
                && hosted_search_requires_client_tools(request, mode)
            {
                if mode == super::native_search::SearchExecutionMode::ProviderNative {
                    return Err(CoreError::Llm(format!(
                        "Provider-native search for {dialect:?} cannot be mixed with client tools on this endpoint. Use Auto, Hybrid, or Nexa Router."
                    )));
                }
                warn!(
                    "Provider-hosted search for {dialect:?} cannot mix client tools; using Nexa Router"
                );
                fallback_request = without_native_search_marker(request);
                &fallback_request
            } else {
                match self
                    .complete_hosted_search(request, dialect, mode, capability)
                    .await
                {
                    Ok(response) => return Ok(response),
                    Err(error)
                        if matches!(
                            mode,
                            super::native_search::SearchExecutionMode::Auto
                                | super::native_search::SearchExecutionMode::Hybrid
                        ) =>
                    {
                        warn!(
                        "Provider-hosted search failed for {dialect:?}; falling back to Nexa Router: {error}"
                    );
                        fallback_request = without_native_search_marker(request);
                        &fallback_request
                    }
                    Err(error) => return Err(error),
                }
            }
        } else {
            request
        };
        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key()?;
        let body = build_request_body_with_config(request, false, Some(&self.config));
        let body_bytes = serialized_json_body(&body, "OpenAI completion request")?;

        let mut attempt = 1;
        let oai: OaiResponse = loop {
            let response = match with_request_timeout(
                apply_openrouter_headers(
                    self.transport
                        .client()
                        .post(&url)
                        .header("Authorization", format!("Bearer {api_key}"))
                        .header("Content-Type", "application/json")
                        .body(body_bytes.clone()),
                    &self.config,
                ),
                self.request_timeout,
            )
            .send()
            .await
            .inspect_err(|error| {
                self.transport.record_transport_failure(&error.to_string());
            }) {
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

            match response.json().await.inspect_err(|error| {
                self.transport.record_transport_failure(&error.to_string());
            }) {
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
        self.transport.record_transport_success();

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
            .map(|usage| {
                usage_from_oai_usage_with_route(
                    usage,
                    oai.id.clone(),
                    oai.model.clone(),
                    oai.openrouter_metadata.clone(),
                )
            })
            .unwrap_or_default();

        let (content, content_thinking) = choice
            .message
            .content
            .as_ref()
            .map(super::streaming::partition_openai_content)
            .unwrap_or_default();
        let (content, tagged_thinking) = super::streaming::partition_complete_think_tags(&content);
        let thinking = choice
            .message
            .reasoning_content
            .or_else(|| choice.message.reasoning.and_then(reasoning_value_to_text))
            .or_else(|| {
                choice
                    .message
                    .reasoning_details
                    .and_then(reasoning_value_to_text)
            })
            .or(content_thinking)
            .or(tagged_thinking);

        Ok(CompletionResponse {
            content,
            tool_calls,
            finish_reason,
            usage,
            thinking,
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        if hosted_search_context(request).is_some() {
            let response = self.complete(request).await?;
            return Ok(Box::pin(futures::stream::iter(
                completion_response_to_stream_chunks(response),
            )));
        }
        if requires_non_streaming_fallback(&request.model) {
            let response = self.complete(request).await?;
            return Ok(Box::pin(futures::stream::iter(
                completion_response_to_stream_chunks(response),
            )));
        }

        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key()?;
        let body = build_request_body_with_config(request, true, Some(&self.config));
        let body_bytes = serialized_json_body(&body, "OpenAI stream request")?;

        info!("OpenAI stream request to {url}, model={}", request.model);
        debug!("Request body: {} bytes", body_bytes.len());

        let response = send_stream_start_request(
            apply_openrouter_headers(
                self.transport
                    .client()
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("Content-Type", "application/json")
                    .body(body_bytes),
                &self.config,
            ),
            self.request_timeout,
            "OpenAI stream request",
        )
        .await
        .inspect_err(|e| {
            self.transport.record_transport_failure(&e.to_string());
            error!("Stream send failed: {e}");
        })?;

        info!("Stream response status: {}", response.status());
        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);
        info!("SSE stream started");

        let transport = Arc::clone(&self.transport);
        tokio::spawn(async move {
            if let Err(e) = parse_sse_stream(response, tx.clone()).await {
                transport.record_transport_failure(&e.to_string());
                error!("SSE stream error: {e}");
                let _ = tx.send(Err(e)).await;
            } else {
                transport.record_transport_success();
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

    fn endpoint_config(provider_type: ProviderType, base_url: &str) -> ProviderConfig {
        ProviderConfig {
            provider_type,
            api_key: Some("test-key".to_string()),
            base_url: Some(base_url.to_string()),
            org_id: None,
            timeout_secs: None,
        }
    }

    fn endpoint_reasoning_request(model: &str) -> CompletionRequest {
        CompletionRequest {
            model: model.to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
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

    fn cacheable_message(
        role: Role,
        content: impl Into<String>,
        stability: super::PromptStability,
        boundary: super::CacheBoundaryHint,
    ) -> Message {
        Message::text(role, content).with_prompt_cache_hint(stability, boundary)
    }

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
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn moonshot_k3_defaults_to_max_and_never_invents_a_token_budget() {
        let request = endpoint_reasoning_request("kimi-k3");
        let config = endpoint_config(ProviderType::Moonshot, "https://api.moonshot.ai/v1");

        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();

        assert_eq!(body["reasoning_effort"], "max");
        assert!(body.get("thinking_budget").is_none());
        assert!(body.get("enable_thinking").is_none());
        assert!(body.get("thinking").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn alibaba_routed_k3_accepts_only_max_and_preserves_thinking() {
        let mut request = endpoint_reasoning_request("kimi/kimi-k3");
        request.reasoning_effort = Some(ReasoningEffort::High);
        request.thinking_budget = Some(20_000);
        let config = endpoint_config(
            ProviderType::AlibabaModelStudio,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
        );

        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();

        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("thinking_budget").is_none());
        assert_eq!(body["preserve_thinking"], true);

        request.reasoning_effort = Some(ReasoningEffort::Max);
        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();
        assert_eq!(body["reasoning_effort"], "max");
    }

    #[test]
    fn qwen38_effort_aliases_and_budget_are_mutually_exclusive_on_token_plan() {
        let mut request = endpoint_reasoning_request("qwen3.8-max");
        request.reasoning_enabled = Some(true);
        request.reasoning_effort = Some(ReasoningEffort::Max);
        request.thinking_budget = Some(32_768);
        let config = endpoint_config(
            ProviderType::Qwen,
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        );

        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();
        assert_eq!(body["enable_thinking"], true);
        assert_eq!(body["reasoning_effort"], "xhigh");
        assert!(body.get("thinking_budget").is_none());

        request.reasoning_effort = None;
        request.thinking_budget = Some(300_000);
        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["thinking_budget"], 262_144);

        request.reasoning_enabled = Some(false);
        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();
        assert_eq!(body["enable_thinking"], false);
        assert!(body.get("thinking_budget").is_none());

        request.model = "qwen3.8-max-preview".to_string();
        request.reasoning_effort = None;
        request.thinking_budget = None;
        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();
        assert_eq!(body["reasoning_effort"], "xhigh");
        assert!(body.get("enable_thinking").is_none());
    }

    #[test]
    fn provider_specific_thinking_objects_do_not_leak_between_models() {
        let moonshot = endpoint_config(ProviderType::Moonshot, "https://api.moonshot.ai/v1");
        let mut k26 = endpoint_reasoning_request("kimi-k2.6");
        k26.reasoning_enabled = Some(true);
        let body =
            serde_json::to_value(build_request_body_with_config(&k26, false, Some(&moonshot)))
                .unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["keep"], "all");

        let alibaba = endpoint_config(
            ProviderType::AlibabaModelStudio,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
        );
        let mut minimax = endpoint_reasoning_request("MiniMax/MiniMax-M3");
        minimax.reasoning_enabled = Some(true);
        let body = serde_json::to_value(build_request_body_with_config(
            &minimax,
            false,
            Some(&alibaba),
        ))
        .unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body.get("reasoning_effort").is_none());

        minimax.reasoning_enabled = Some(false);
        let body = serde_json::to_value(build_request_body_with_config(
            &minimax,
            false,
            Some(&alibaba),
        ))
        .unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn curated_direct_endpoints_emit_only_their_verified_reasoning_controls() {
        let mut xai_request = endpoint_reasoning_request("grok-4.3");
        xai_request.reasoning_effort = Some(ReasoningEffort::None);
        let xai = endpoint_config(ProviderType::OpenAi, "https://api.x.ai/v1");
        let body = serde_json::to_value(build_request_body_with_config(
            &xai_request,
            false,
            Some(&xai),
        ))
        .unwrap();
        assert_eq!(body["reasoning_effort"], "none");

        let mut mistral_request = endpoint_reasoning_request("mistral-medium-3-5");
        mistral_request.reasoning_effort = Some(ReasoningEffort::High);
        let mistral = endpoint_config(ProviderType::OpenAi, "https://api.mistral.ai/v1");
        let body = serde_json::to_value(build_request_body_with_config(
            &mistral_request,
            false,
            Some(&mistral),
        ))
        .unwrap();
        assert_eq!(body["reasoning_effort"], "high");

        let mut multi_agent = endpoint_reasoning_request("grok-4.20-multi-agent-0309");
        multi_agent.reasoning_effort = Some(ReasoningEffort::High);
        let body = serde_json::to_value(build_request_body_with_config(
            &multi_agent,
            false,
            Some(&xai),
        ))
        .unwrap();
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn direct_reasoning_history_uses_each_providers_documented_content_shape() {
        let assistant = Message {
            role: Role::Assistant,
            parts: vec![ContentPart::Text {
                text: "final answer".to_string(),
            }],
            name: None,
            tool_calls: None,
            reasoning_content: Some("work it out".to_string()),
            prompt_cache_hint: None,
        };

        let minimax_request = CompletionRequest {
            messages: vec![assistant.clone()],
            ..endpoint_reasoning_request("MiniMax-M3")
        };
        let minimax = endpoint_config(ProviderType::OpenAi, "https://api.minimax.io/v1");
        let body = serde_json::to_value(build_request_body_with_config(
            &minimax_request,
            false,
            Some(&minimax),
        ))
        .unwrap();
        assert_eq!(
            body["messages"][0]["content"],
            "<think>\nwork it out\n</think>\nfinal answer"
        );
        assert!(body["messages"][0].get("reasoning_content").is_none());

        let mistral_request = CompletionRequest {
            messages: vec![assistant],
            ..endpoint_reasoning_request("mistral-medium-3-5")
        };
        let mistral = endpoint_config(ProviderType::OpenAi, "https://api.mistral.ai/v1");
        let body = serde_json::to_value(build_request_body_with_config(
            &mistral_request,
            false,
            Some(&mistral),
        ))
        .unwrap();
        assert_eq!(body["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(
            body["messages"][0]["content"][0]["thinking"][0]["text"],
            "work it out"
        );
        assert_eq!(body["messages"][0]["content"][1]["text"], "final answer");
    }

    #[test]
    fn unknown_endpoint_emits_no_intermediary_reasoning_fields() {
        let mut request = endpoint_reasoning_request("kimi/kimi-k3");
        request.reasoning_enabled = Some(true);
        request.reasoning_effort = Some(ReasoningEffort::Max);
        request.thinking_budget = Some(20_000);
        let config = endpoint_config(
            ProviderType::AlibabaModelStudio,
            "https://tenant.example.test/v1",
        );

        let body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&config),
        ))
        .unwrap();
        for field in [
            "reasoning_effort",
            "thinking_budget",
            "enable_thinking",
            "thinking",
            "preserve_thinking",
        ] {
            assert!(body.get(field).is_none(), "unexpected field {field}");
        }
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
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenAi),
            routing_session_id: None,
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
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::Max),
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
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
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_completion_tokens").is_none());
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
            prompt_cache_hint: None,
        };
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello"), assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
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
            prompt_cache_hint: None,
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
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
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
            prompt_cache_hint: None,
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
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
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
            prompt_cache_hint: None,
        };
        let request = CompletionRequest {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello"), assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: Some(false),
            reasoning_effort: None,
            provider_type: Some(ProviderType::DeepSeek),
            routing_session_id: None,
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
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenAi),
            routing_session_id: None,
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
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::OpenAi),
            routing_session_id: None,
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
    fn openrouter_reasoning_uses_nested_reasoning_parameter() {
        let request = CompletionRequest {
            model: "x-ai/grok-4.3".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::OpenRouter),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("temperature").is_some());
    }

    #[test]
    fn openrouter_reasoning_can_use_token_budget() {
        let request = CompletionRequest {
            model: "anthropic/claude-sonnet-4.6".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: Some(2048),
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenRouter),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["reasoning"]["max_tokens"], 2048);
        assert!(body["reasoning"].get("effort").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn openrouter_reasoning_details_text_is_extracted() {
        let value = serde_json::json!([
            { "type": "reasoning.text", "text": "first " },
            { "type": "reasoning.summary", "summary_text": "second" }
        ]);

        assert_eq!(
            reasoning_value_to_text(value).as_deref(),
            Some("first second")
        );
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
            prompt_cache_hint: None,
        };
        let request = CompletionRequest {
            model: "qwen3-coder".to_string(),
            messages: vec![assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
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
    fn alibaba_qwen_models_use_raw_tool_arguments_without_affecting_router_models() {
        assert!(requires_raw_tool_arguments(
            "qwen3.7-max",
            Some(&ProviderType::AlibabaModelStudio),
        ));
        assert!(requires_raw_tool_arguments(
            "qwq-plus",
            Some(&ProviderType::AlibabaModelStudio),
        ));
        assert!(!requires_raw_tool_arguments(
            "deepseek-v4",
            Some(&ProviderType::AlibabaModelStudio),
        ));
    }

    #[test]
    fn qwen_thinking_request_uses_dashscope_extra_body_fields() {
        let request = CompletionRequest {
            model: "qwen3.6-plus".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: Some(2048),
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, true)).unwrap();

        assert_eq!(body["enable_thinking"], true);
        assert_eq!(body["thinking_budget"], 2048);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["max_tokens"], 100);
    }

    #[test]
    fn intermediary_reasoning_fields_require_an_endpoint_scoped_profile() {
        let request_for = |provider_type| CompletionRequest {
            model: "deepseek-ai/DeepSeek-V3.2".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: Some(4096),
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(provider_type),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let alibaba = serde_json::to_value(build_request_body(
            &request_for(ProviderType::AlibabaModelStudio),
            false,
        ))
        .unwrap();
        assert!(alibaba.get("enable_thinking").is_none());
        assert!(alibaba.get("thinking_budget").is_none());

        let siliconflow = serde_json::to_value(build_request_body(
            &request_for(ProviderType::SiliconFlow),
            false,
        ))
        .unwrap();
        assert_eq!(siliconflow["enable_thinking"], true);
        assert_eq!(siliconflow["thinking_budget"], 4096);
        assert!(siliconflow.get("thinking").is_none());
    }

    #[test]
    fn qwen_thinking_does_not_send_openai_reasoning_effort() {
        let request = CompletionRequest {
            model: "qwen3.6-plus".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, true)).unwrap();

        assert_eq!(body["enable_thinking"], true);
        assert!(body.get("thinking_budget").is_none());
        assert!(body.get("reasoning_effort").is_none());
        let temperature = body["temperature"].as_f64().expect("temperature");
        assert!((temperature - 0.4).abs() < 1e-6);
        assert_eq!(body["max_tokens"], 100);
    }

    #[test]
    fn qwen_thinking_replays_real_reasoning_content_without_placeholder() {
        let assistant_with_reasoning = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: "{\"query\":\"x\"}".to_string(),
                thought_signature: None,
            }]),
            reasoning_content: Some("need lookup".to_string()),
            prompt_cache_hint: None,
        };
        let assistant_without_reasoning = Message {
            role: Role::Assistant,
            parts: vec![],
            name: None,
            tool_calls: None,
            reasoning_content: None,
            prompt_cache_hint: None,
        };
        let request = CompletionRequest {
            model: "qwen3.6-plus".to_string(),
            messages: vec![assistant_with_reasoning, assistant_without_reasoning],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: Some(2048),
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["messages"][0]["reasoning_content"], "need lookup");
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn qwen_requests_add_stable_prompt_cache_markers() {
        let request = CompletionRequest {
            model: "qwen3.7-max".to_string(),
            messages: vec![
                cacheable_message(
                    Role::System,
                    "stable system ".repeat(400),
                    super::PromptStability::Stable,
                    super::CacheBoundaryHint::PolicyEnd,
                ),
                cacheable_message(
                    Role::System,
                    "retrieved evidence",
                    super::PromptStability::Replayable,
                    super::CacheBoundaryHint::StableEvidenceEnd,
                ),
                cacheable_message(
                    Role::User,
                    "hello",
                    super::PromptStability::Replayable,
                    super::CacheBoundaryHint::ReplayableTurnTail,
                ),
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
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert!(body["messages"][1]["content"][0]["cache_control"].is_object());
        assert!(body["messages"][2]["content"][0]["cache_control"].is_object());
        assert!(body["tools"][0].get("cache_control").is_none());
    }

    #[test]
    fn alibaba_qwen_models_keep_cache_markers_without_affecting_router_models() {
        let request_for = |model: &str| CompletionRequest {
            model: model.to_string(),
            messages: vec![cacheable_message(
                Role::System,
                "stable system ".repeat(400),
                super::PromptStability::Stable,
                super::CacheBoundaryHint::PolicyEnd,
            )],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: Some(vec![ToolDefinition {
                name: "search".into(),
                description: "Search".into(),
                parameters: serde_json::json!({"type":"object"}),
            }]),
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::AlibabaModelStudio),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let qwen =
            serde_json::to_value(build_request_body(&request_for("qwen3.7-max"), false)).unwrap();
        assert_eq!(
            qwen["messages"][0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert!(qwen["tools"][0].get("cache_control").is_none());

        let third_party =
            serde_json::to_value(build_request_body(&request_for("kimi-k2.7-code"), false))
                .unwrap();
        assert!(third_party["messages"][0]["content"].is_string());
        assert!(third_party["tools"][0].get("cache_control").is_none());
    }

    #[test]
    fn qwen_cache_markers_target_message_boundaries_not_tool_definitions() {
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }]);
        let mut tool = cacheable_message(
            Role::Tool,
            "tool result",
            super::PromptStability::Replayable,
            super::CacheBoundaryHint::LatestToolRound,
        );
        tool.name = Some("call-1".to_string());
        let request = CompletionRequest {
            model: "qwen3.7-max".to_string(),
            messages: vec![
                cacheable_message(
                    Role::System,
                    "stable system ".repeat(400),
                    super::PromptStability::Stable,
                    super::CacheBoundaryHint::PolicyEnd,
                ),
                cacheable_message(
                    Role::User,
                    "original request",
                    super::PromptStability::Replayable,
                    super::CacheBoundaryHint::ReplayableTurnTail,
                ),
                assistant,
                tool,
            ],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][2].get("cache_control").is_none());
        assert!(body["messages"][3]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn non_leading_system_messages_are_sent_as_user_context() {
        let request = CompletionRequest {
            model: "gpt-5.1".to_string(),
            messages: vec![
                Message::text(Role::System, "stable system"),
                Message::text(Role::User, "question"),
                Message::text(Role::System, "runtime tail"),
            ],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenAi),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"], "runtime tail");
    }

    #[test]
    fn direct_openai_requests_include_stable_prompt_cache_key() {
        let tool = ToolDefinition {
            name: "search".into(),
            description: "Search".into(),
            parameters: serde_json::json!({"type":"object"}),
        };
        let first = CompletionRequest {
            model: "gpt-5.1".to_string(),
            messages: vec![
                Message::text(Role::System, "stable system"),
                Message::text(Role::System, "runtime one"),
                Message::text(Role::User, "hello"),
            ],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: Some(vec![tool.clone()]),
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::OpenAi),
            routing_session_id: None,
            parallel_tool_calls: true,
        };
        let second = CompletionRequest {
            messages: vec![
                Message::text(Role::System, "stable system"),
                Message::text(Role::System, "runtime two"),
                Message::text(Role::User, "different user input"),
            ],
            ..first.clone()
        };

        let first_body = serde_json::to_value(build_request_body(&first, false)).unwrap();
        let second_body = serde_json::to_value(build_request_body(&second, false)).unwrap();
        let key = first_body["prompt_cache_key"].as_str().expect("cache key");

        assert!(key.starts_with("nexa-"));
        assert!(key.len() <= 64);
        assert_eq!(
            first_body["prompt_cache_key"],
            second_body["prompt_cache_key"]
        );
    }

    #[test]
    fn custom_openai_compatible_requests_do_not_send_prompt_cache_key() {
        let request = CompletionRequest {
            model: "custom-model".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Custom),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn qwen_markers_require_a_trusted_endpoint_and_minimum_prompt() {
        let request = CompletionRequest {
            model: "qwen3.8-max".to_string(),
            messages: vec![cacheable_message(
                Role::System,
                "stable system ".repeat(400),
                super::PromptStability::Stable,
                super::CacheBoundaryHint::PolicyEnd,
            )],
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
            parallel_tool_calls: true,
            ..CompletionRequest::default()
        };
        let unknown = ProviderConfig {
            provider_type: ProviderType::Qwen,
            base_url: Some("https://example.com/v1".to_string()),
            api_key: None,
            org_id: None,
            timeout_secs: None,
        };
        let trusted = ProviderConfig {
            provider_type: ProviderType::Qwen,
            base_url: Some(
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1".to_string(),
            ),
            api_key: None,
            org_id: None,
            timeout_secs: None,
        };

        let unknown_body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&unknown),
        ))
        .unwrap();
        let trusted_body = serde_json::to_value(build_request_body_with_config(
            &request,
            false,
            Some(&trusted),
        ))
        .unwrap();
        let short_body = serde_json::to_value(build_request_body(
            &CompletionRequest {
                messages: vec![Message::text(Role::System, "short")],
                ..request.clone()
            },
            false,
        ))
        .unwrap();
        let unhinted_body = serde_json::to_value(build_request_body_with_config(
            &CompletionRequest {
                messages: vec![Message::text(Role::System, "stable system ".repeat(400))],
                ..request.clone()
            },
            false,
            Some(&trusted),
        ))
        .unwrap();

        assert!(unknown_body["messages"][0]["content"].is_string());
        assert!(trusted_body["messages"][0]["content"][0]["cache_control"].is_object());
        assert!(short_body["messages"][0]["content"].is_string());
        assert!(unhinted_body["messages"][0]["content"].is_string());
    }

    #[test]
    fn openrouter_request_uses_privacy_preserving_session_affinity() {
        let request = CompletionRequest {
            model: "anthropic/claude-sonnet-4.6".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            provider_type: Some(ProviderType::OpenRouter),
            routing_session_id: Some("nexa-deadbeef".to_string()),
            parallel_tool_calls: true,
            ..CompletionRequest::default()
        };

        let body = serde_json::to_value(build_request_body(&request, false)).unwrap();

        assert_eq!(body["session_id"], "nexa-deadbeef");
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
        assert_eq!(details.cache_creation_tokens(), Some(32));
    }

    #[test]
    fn openai_compatible_usage_maps_top_level_prompt_cache_hits() {
        let usage: OaiUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_cache_hit_tokens": 48,
            "prompt_cache_miss_tokens": 52
        }))
        .unwrap();

        let normalized = usage_from_oai_usage(usage);

        assert_eq!(normalized.cache_read_tokens, Some(48));
        assert_eq!(normalized.cache_miss_tokens, Some(52));
        assert_eq!(
            normalized.provider_raw.as_ref().and_then(|raw| raw
                .pointer("/usage/prompt_cache_hit_tokens")
                .and_then(serde_json::Value::as_u64)),
            Some(48)
        );
    }

    #[test]
    fn openai_compatible_usage_maps_nested_qwen_cache_creation() {
        let usage: OaiUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 120,
            "completion_tokens": 20,
            "total_tokens": 140,
            "prompt_tokens_details": {
                "cached_tokens": 64,
                "cache_creation": {
                    "cache_creation_input_tokens": 24,
                    "cache_type": "ephemeral"
                }
            }
        }))
        .unwrap();

        let normalized = usage_from_oai_usage(usage);

        assert_eq!(normalized.cache_read_tokens, Some(64));
        assert_eq!(normalized.cache_creation_tokens, Some(24));
        assert_eq!(
            normalized.provider_raw.as_ref().and_then(|raw| raw
                .pointer("/usage/prompt_tokens_details/cache_creation/cache_creation_input_tokens")
                .and_then(serde_json::Value::as_u64)),
            Some(24)
        );
    }

    #[test]
    fn openai_compatible_usage_maps_flat_cache_creation_aliases() {
        let usage: OaiUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 120,
            "completion_tokens": 20,
            "total_tokens": 140,
            "prompt_tokens_details": {
                "cached_tokens": 64,
                "cache_creation_input_tokens": 24
            }
        }))
        .unwrap();

        let normalized = usage_from_oai_usage(usage);

        assert_eq!(normalized.cache_read_tokens, Some(64));
        assert_eq!(normalized.cache_creation_tokens, Some(24));
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
            prompt_cache_hint: None,
        };
        let request = CompletionRequest {
            model: "qwen3-coder".to_string(),
            messages: vec![assistant],
            temperature: Some(0.4),
            max_tokens: Some(100),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: Some(ProviderType::Qwen),
            routing_session_id: None,
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
                tool_prompt_tokens: None,
                cache_read_tokens: None,
                cache_miss_tokens: None,
                cache_creation_tokens: None,
                provider_raw: None,
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

    #[test]
    fn responses_request_replaces_only_local_search_in_auto_mode() {
        let plan = super::super::native_search::NativeSearchPlan::resolve(
            super::super::native_search::SearchExecutionMode::Auto,
            ProviderType::OpenAi,
            Some("https://api.openai.com/v1"),
            "gpt-5.6",
        );
        let mut request = endpoint_reasoning_request("gpt-5.6");
        request.tools = Some(vec![
            ToolDefinition {
                name: super::super::native_search::LOCAL_WEB_SEARCH_TOOL.to_string(),
                description: "local".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "read_file".to_string(),
                description: "read".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            plan.marker().expect("trusted marker"),
        ]);
        let (dialect, mode, capability) = hosted_search_context(&request).expect("context");
        let body = build_responses_request(&request, dialect, mode, capability);
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(body["tools"][1]["name"], "read_file");
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn responses_tool_loop_replays_encrypted_reasoning_before_function_state() {
        let capability = crate::model_catalog::NativeWebSearchCapability {
            dialect: super::super::native_search::NativeSearchDialect::OpenAiResponses,
            supports_domains: true,
            supports_recency: false,
            supports_locale: false,
            supports_location: true,
            supports_citations: true,
            supports_stream_events: true,
            can_mix_client_tools: true,
        };
        let response = parse_responses_completion(
            serde_json::json!({
                "status": "completed",
                "output": [
                    {
                        "type": "reasoning",
                        "id": "rs_1",
                        "encrypted_content": "encrypted-reasoning",
                        "summary": []
                    },
                    {
                        "type": "function_call",
                        "id": "fc_1",
                        "call_id": "call_1",
                        "name": "read_file",
                        "arguments": "{\"path\":\"README.md\"}"
                    }
                ]
            }),
            super::super::native_search::NativeSearchDialect::OpenAiResponses,
            capability,
        )
        .unwrap();
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.parts.clear();
        assistant.tool_calls = response.tool_calls;
        let tool_result = Message::text_with_name(Role::Tool, "contents", "call_1");

        let replay = responses_input_items(&[assistant, tool_result]);
        assert_eq!(replay[0]["type"], "reasoning");
        assert_eq!(replay[0]["encrypted_content"], "encrypted-reasoning");
        assert_eq!(replay[1]["type"], "function_call");
        assert_eq!(replay[2]["type"], "function_call_output");
    }

    #[test]
    fn responses_without_client_tool_support_require_router_for_mixed_rounds() {
        let mut request = endpoint_reasoning_request("deepseek-v4-pro");
        request.tools = Some(vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "read".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }]);
        assert!(hosted_search_requires_client_tools(
            &request,
            super::super::native_search::SearchExecutionMode::Auto,
        ));
    }

    #[test]
    fn responses_parser_normalizes_openai_citations_but_not_deepseek_guesses() {
        let payload = serde_json::json!({
            "status": "completed",
            "output": [
                { "type": "web_search_call", "action": { "type": "search", "query": "Nexa" } },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "Answer",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://example.com/evidence",
                            "title": "Evidence",
                            "start_index": 0,
                            "end_index": 6
                        }]
                    }]
                }
            ],
            "usage": { "input_tokens": 4, "output_tokens": 3, "total_tokens": 7 }
        });
        let openai_capability = crate::model_catalog::NativeWebSearchCapability {
            dialect: super::super::native_search::NativeSearchDialect::OpenAiResponses,
            supports_domains: true,
            supports_recency: false,
            supports_locale: false,
            supports_location: true,
            supports_citations: true,
            supports_stream_events: true,
            can_mix_client_tools: true,
        };
        let openai = parse_responses_completion(
            payload.clone(),
            super::super::native_search::NativeSearchDialect::OpenAiResponses,
            openai_capability,
        )
        .unwrap();
        assert!(openai
            .content
            .contains("[Evidence](https://example.com/evidence)"));
        assert_eq!(openai.usage.total_tokens, 7);

        let deepseek_capability = crate::model_catalog::NativeWebSearchCapability {
            dialect: super::super::native_search::NativeSearchDialect::DeepSeekResponses,
            supports_domains: false,
            supports_recency: false,
            supports_locale: false,
            supports_location: false,
            supports_citations: false,
            supports_stream_events: true,
            can_mix_client_tools: true,
        };
        let deepseek = parse_responses_completion(
            payload,
            super::super::native_search::NativeSearchDialect::DeepSeekResponses,
            deepseek_capability,
        )
        .unwrap();
        assert_eq!(deepseek.content, "Answer");
    }
}
