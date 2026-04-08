//! OpenAI-compatible LLM provider.
//!
//! Also used for DeepSeek, LM Studio, Azure OpenAI, and custom endpoints
//! that expose the same `/v1/chat/completions` interface.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use super::{
    streaming::parse_sse_stream, CompletionRequest, CompletionResponse, ContentPart, FinishReason,
    LlmProvider, Message, ProviderConfig, ProviderType, Role, StreamChunk, ToolCallRequest,
    ToolDefinition, Usage,
};
use crate::error::CoreError;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT_SECS: u64 = 600;

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
    Text { text: String },
    ImageUrl { image_url: OaiImageUrl },
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
    arguments: String,
}

#[derive(Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiToolFunction,
}

#[derive(Serialize)]
struct OaiToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
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
    arguments: String,
}

#[derive(Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    completion_tokens_details: Option<OaiCompletionTokensDetails>,
}

#[derive(Deserialize)]
struct OaiCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
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

/// Check if the model is an OpenAI reasoning model (o1/o3/o4 series).
fn is_reasoning_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

/// Check if the model is a DeepSeek reasoner.
fn is_deepseek_reasoner(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("deepseek-reasoner") || m.contains("deepseek-r1")
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

fn convert_message(msg: &Message, include_reasoning_content: bool) -> OaiMessage {
    let has_images = msg.has_images();

    // Build content: use array format when images are present, plain string otherwise.
    let content: Option<OaiContent> = if has_images {
        let parts: Vec<OaiContentPart> = msg
            .parts
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => OaiContentPart::Text { text: text.clone() },
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
                .map(|tc| OaiToolCallOut {
                    id: tc.id.clone(),
                    call_type: "function".to_string(),
                    function: OaiFunctionOut {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
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
        oai.reasoning_content = msg.reasoning_content.clone();
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
        })
        .collect()
}

fn build_request_body(request: &CompletionRequest, stream: bool) -> OaiRequest {
    let is_reasoning = is_reasoning_model(&request.model);
    let is_deepseek = is_deepseek_reasoner(&request.model);
    let model_lower = request.model.to_lowercase();
    let is_deepseek_provider = matches!(request.provider_type, Some(ProviderType::DeepSeek))
        || model_lower.contains("deepseek");
    let is_deepseek_chat = model_lower.contains("deepseek-chat");
    let deepseek_thinking_enabled =
        is_deepseek_provider && is_deepseek_chat && request.thinking_budget.is_some();
    let include_reasoning_content = is_deepseek_provider;
    let needs_completion_tokens = is_reasoning || is_deepseek;
    let suppress_temperature = is_reasoning || is_deepseek;

    OaiRequest {
        model: request.model.clone(),
        messages: request
            .messages
            .iter()
            .map(|m| convert_message(m, include_reasoning_content))
            .collect(),
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
        reasoning_effort: if is_reasoning {
            Some(
                request
                    .reasoning_effort
                    .as_ref()
                    .map(|r| r.to_string())
                    .unwrap_or_else(|| "medium".to_string()),
            )
        } else {
            None
        },
        thinking: if deepseek_thinking_enabled {
            Some(OaiThinking {
                thinking_type: "enabled".to_string(),
            })
        } else {
            None
        },
        tools: request.tools.as_ref().map(|t| convert_tools(t)),
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
}

impl OpenAiProvider {
    /// Create a new provider with an async reqwest client.
    pub fn new(config: ProviderConfig) -> Result<Self, CoreError> {
        let timeout = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(timeout))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .pool_max_idle_per_host(5)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| CoreError::Llm(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self { client, config })
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

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
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

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let oai: OaiResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse completion response: {e}")))?;

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
                    arguments: tc.function.arguments,
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
            .map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                thinking_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
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
        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key()?;
        let body = build_request_body(request, true);

        info!("OpenAI stream request to {url}, model={}", request.model);
        let body_json = serde_json::to_string(&body).unwrap_or_default();
        debug!("Request body: {} bytes", body_json.len());

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                error!("Stream send failed: {e}");
                if e.is_connect() || e.is_timeout() {
                    CoreError::TransientLlm(format!("Request failed: {e}"))
                } else {
                    CoreError::Llm(format!("Request failed: {e}"))
                }
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
