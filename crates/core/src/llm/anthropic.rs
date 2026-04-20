//! Anthropic Claude LLM provider.
//!
//! Implements the Anthropic Messages API which has a different format from
//! OpenAI: system prompts are top-level, tool schemas use `input_schema`,
//! and streaming uses named SSE events.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{error, info};

use super::{
    CompletionRequest, CompletionResponse, ContentPart, FinishReason, LlmProvider, Message,
    ProviderConfig, Role, StreamChunk, ToolCallDelta, ToolCallRequest, ToolDefinition, Usage,
};
use crate::conversation::memory::estimate_tokens;
use crate::error::CoreError;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_TOKENS: u32 = 4096;

// ---------------------------------------------------------------------------
// Anthropic API wire types — request
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicThinking {
    r#type: String,
    budget_tokens: u32,
}

#[derive(Clone, Serialize)]
struct CacheControl {
    r#type: String,
}

#[derive(Serialize)]
struct AnthropicSystemBlock {
    r#type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<AnthropicSystemBlock>>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Anthropic content can be a plain string or an array of content blocks.
#[derive(Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct AnthropicImageSource {
    r#type: String,
    media_type: String,
    data: String,
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

// ---------------------------------------------------------------------------
// Anthropic API wire types — response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseBlock>,
    stop_reason: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum AnthropicResponseBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Thinking {
        thinking: String,
    },
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicErrorBody,
}

#[derive(Deserialize)]
struct AnthropicErrorBody {
    message: String,
}

// ---------------------------------------------------------------------------
// Streaming wire types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum AnthropicStreamEvent {
    MessageStart {
        message: AnthropicStreamMessage,
    },
    ContentBlockStart {
        #[allow(dead_code)]
        index: usize,
        content_block: AnthropicStreamContentBlock,
    },
    ContentBlockDelta {
        #[allow(dead_code)]
        index: usize,
        delta: AnthropicStreamDelta,
    },
    ContentBlockStop {
        #[allow(dead_code)]
        index: usize,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicDeltaUsage>,
    },
    MessageStop,
    Ping,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct AnthropicStreamMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum AnthropicStreamContentBlock {
    Text {
        #[allow(dead_code)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
    Thinking {
        #[allow(dead_code)]
        thinking: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum AnthropicStreamDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
}

#[derive(Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicDeltaUsage {
    output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "end_turn" => FinishReason::Stop,
        "tool_use" => FinishReason::ToolCalls,
        "max_tokens" => FinishReason::Length,
        "stop_sequence" => FinishReason::Stop,
        _ => FinishReason::Other,
    }
}

/// Convert our unified messages to Anthropic format.
///
/// - System messages are extracted and returned separately (Anthropic puts them top-level).
/// - Tool-result messages are wrapped in user messages with `tool_result` content blocks.
/// - Assistant messages with tool_calls become content block arrays.
fn convert_messages(
    messages: &[Message],
) -> (Option<Vec<AnthropicSystemBlock>>, Vec<AnthropicMessage>) {
    let mut system_text: Option<String> = None;
    let mut out: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                // Anthropic only supports a single system prompt; concat if multiple.
                let text = msg.text_content();
                match &mut system_text {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(&text);
                    }
                    None => system_text = Some(text),
                }
            }
            Role::User => {
                if msg.has_images() {
                    let blocks: Vec<AnthropicContentBlock> = msg
                        .parts
                        .iter()
                        .map(|p| match p {
                            ContentPart::Text { text } => {
                                AnthropicContentBlock::Text { text: text.clone() }
                            }
                            ContentPart::Image { media_type, data } => {
                                AnthropicContentBlock::Image {
                                    source: AnthropicImageSource {
                                        r#type: "base64".to_string(),
                                        media_type: media_type.clone(),
                                        data: data.clone(),
                                    },
                                }
                            }
                        })
                        .collect();
                    out.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Blocks(blocks),
                    });
                } else {
                    out.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Text(msg.text_content()),
                    });
                }
            }
            Role::Assistant => {
                if let Some(ref calls) = msg.tool_calls {
                    // Build content blocks: text (if any) + tool_use blocks.
                    let mut blocks = Vec::new();
                    let text = msg.text_content();
                    if !text.is_empty() {
                        blocks.push(AnthropicContentBlock::Text { text });
                    }
                    for tc in calls {
                        let input: serde_json::Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                        blocks.push(AnthropicContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input,
                        });
                    }
                    out.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: AnthropicContent::Blocks(blocks),
                    });
                } else {
                    out.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: AnthropicContent::Text(msg.text_content()),
                    });
                }
            }
            Role::Tool => {
                // Anthropic expects tool results as user messages with tool_result blocks.
                // If the previous message is already a user role with blocks, append.
                let appended = if let Some(last) = out.last_mut() {
                    if last.role == "user" {
                        if let AnthropicContent::Blocks(ref mut blocks) = last.content {
                            blocks.push(AnthropicContentBlock::ToolResult {
                                tool_use_id: msg.name.clone().unwrap_or_default(),
                                content: msg.text_content(),
                            });
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !appended {
                    out.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Blocks(vec![
                            AnthropicContentBlock::ToolResult {
                                tool_use_id: msg.name.clone().unwrap_or_default(),
                                content: msg.text_content(),
                            },
                        ]),
                    });
                }
            }
        }
    }

    // Wrap system text in content blocks with cache_control for prompt caching.
    // Anthropic caches everything up to a cache_control breakpoint, so placing
    // one on the system block ensures the system prompt is cached across turns.
    let system = system_text.map(|text| {
        vec![AnthropicSystemBlock {
            r#type: "text".to_string(),
            text,
            cache_control: Some(CacheControl {
                r#type: "ephemeral".to_string(),
            }),
        }]
    });

    (system, out)
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<AnthropicTool> {
    let len = tools.len();
    tools
        .iter()
        .enumerate()
        .map(|(i, t)| AnthropicTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.parameters.clone(),
            // Place cache_control breakpoint on the last tool definition.
            // Anthropic caches everything UP TO the breakpoint, so this
            // ensures all tool definitions (+ system prompt) are cached.
            cache_control: if i == len - 1 {
                Some(CacheControl {
                    r#type: "ephemeral".to_string(),
                })
            } else {
                None
            },
        })
        .collect()
}

fn build_request_body(
    request: &CompletionRequest,
    system: Option<Vec<AnthropicSystemBlock>>,
    messages: Vec<AnthropicMessage>,
    stream: bool,
) -> AnthropicRequest {
    // NOTE: Anthropic's API returns a clear error for models that don't support
    // thinking, so no model-gating is applied here (unlike Gemini).
    let (thinking, temperature, effective_max_tokens) =
        if let Some(budget) = request.thinking_budget {
            let budget = budget.max(1024); // Anthropic requires budget_tokens >= 1024
            let base_max = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
            // Ensure max_tokens > budget_tokens, with headroom for the response
            let effective_max = base_max.max(budget + 4096);
            (
                Some(AnthropicThinking {
                    r#type: "enabled".to_string(),
                    budget_tokens: budget,
                }),
                None, // Anthropic requires temperature unset when thinking is enabled
                effective_max,
            )
        } else {
            (
                None,
                request.temperature,
                request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            )
        };

    AnthropicRequest {
        model: request.model.clone(),
        max_tokens: effective_max_tokens,
        system,
        messages,
        temperature,
        tools: request.tools.as_ref().map(|t| convert_tools(t)),
        stop_sequences: request.stop.clone(),
        stream: if stream { Some(true) } else { None },
        thinking,
    }
}

// ---------------------------------------------------------------------------
// Anthropic SSE stream parser
// ---------------------------------------------------------------------------

/// Parse Anthropic's SSE stream format.
///
/// Unlike OpenAI, Anthropic uses named `event:` lines followed by `data:` lines.
/// Events include `message_start`, `content_block_start`, `content_block_delta`,
/// `message_delta`, `message_stop`, and `ping`.
async fn parse_anthropic_stream(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, CoreError>>,
) -> Result<(), CoreError> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    // Track input_tokens from message_start for final usage assembly.
    let mut input_tokens: u32 = 0;
    // Track current tool call id/name from content_block_start.
    let mut current_tool_id = String::new();
    let mut current_tool_name: Option<String> = None;
    // Accumulate thinking content for token estimation.
    let mut thinking_text = String::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = chunk_result.map_err(|e| CoreError::Llm(format!("Stream read error: {e}")))?;
        let text = std::str::from_utf8(&chunk)
            .map_err(|e| CoreError::Llm(format!("Invalid UTF-8 in stream: {e}")))?;
        buffer.push_str(text);

        // Process complete event blocks (separated by double newlines).
        while let Some(block_end) = buffer.find("\n\n") {
            let block = buffer[..block_end].to_string();
            buffer = buffer[block_end + 2..].to_string();

            // Extract event type and data from the block.
            let mut event_type = String::new();
            let mut data = String::new();
            for line in block.lines() {
                if let Some(ev) = line.strip_prefix("event: ") {
                    event_type = ev.trim().to_string();
                } else if let Some(d) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    data = d.trim().to_string();
                }
            }

            if data.is_empty() {
                continue;
            }

            // Parse the JSON data based on event type.
            let event: AnthropicStreamEvent = match serde_json::from_str(&data) {
                Ok(ev) => ev,
                Err(e) => {
                    // Skip unparseable events (may be unknown new event types).
                    tracing::debug!("Anthropic SSE parse skip (event={event_type}): {e}");
                    continue;
                }
            };

            match event {
                AnthropicStreamEvent::MessageStart { message } => {
                    if let Some(u) = message.usage {
                        input_tokens = u.input_tokens;
                    }
                }
                AnthropicStreamEvent::ContentBlockStart { content_block, .. } => {
                    match content_block {
                        AnthropicStreamContentBlock::Text { .. } => {
                            current_tool_id.clear();
                            current_tool_name = None;
                        }
                        AnthropicStreamContentBlock::Thinking { .. } => {
                            current_tool_id.clear();
                            current_tool_name = None;
                        }
                        AnthropicStreamContentBlock::ToolUse { id, name } => {
                            current_tool_id = id.clone();
                            current_tool_name = Some(name.clone());
                            // Emit an initial tool call delta with the name.
                            let chunk = StreamChunk {
                                delta: String::new(),
                                tool_call_delta: Some(ToolCallDelta {
                                    id,
                                    name: Some(name),
                                    arguments_delta: String::new(),
                                    index: None,
                                    thought_signature: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                thinking_delta: None,
                            };
                            if tx.send(Ok(chunk)).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                }
                AnthropicStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                    AnthropicStreamDelta::TextDelta { text } => {
                        let chunk = StreamChunk {
                            delta: text,
                            tool_call_delta: None,
                            finish_reason: None,
                            usage: None,
                            thinking_delta: None,
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            return Ok(());
                        }
                    }
                    AnthropicStreamDelta::ThinkingDelta { thinking } => {
                        thinking_text.push_str(&thinking);
                        let chunk = StreamChunk {
                            delta: String::new(),
                            tool_call_delta: None,
                            finish_reason: None,
                            usage: None,
                            thinking_delta: Some(thinking),
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            return Ok(());
                        }
                    }
                    AnthropicStreamDelta::InputJsonDelta { partial_json } => {
                        let chunk = StreamChunk {
                            delta: String::new(),
                            tool_call_delta: Some(ToolCallDelta {
                                id: current_tool_id.clone(),
                                name: current_tool_name.clone(),
                                arguments_delta: partial_json,
                                index: None,
                                thought_signature: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            thinking_delta: None,
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            return Ok(());
                        }
                    }
                },
                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                    let finish = delta.stop_reason.as_deref().map(parse_finish_reason);
                    let estimated_thinking = if !thinking_text.is_empty() {
                        Some(estimate_tokens(&thinking_text))
                    } else {
                        None
                    };
                    let usage_info = usage.map(|u| Usage {
                        prompt_tokens: input_tokens,
                        completion_tokens: u.output_tokens,
                        total_tokens: input_tokens + u.output_tokens,
                        thinking_tokens: estimated_thinking,
                    });
                    let chunk = StreamChunk {
                        delta: String::new(),
                        tool_call_delta: None,
                        finish_reason: finish,
                        usage: usage_info,
                        thinking_delta: None,
                    };
                    if tx.send(Ok(chunk)).await.is_err() {
                        return Ok(());
                    }
                }
                AnthropicStreamEvent::MessageStop => {
                    return Ok(());
                }
                AnthropicStreamEvent::ContentBlockStop { .. }
                | AnthropicStreamEvent::Ping
                | AnthropicStreamEvent::Unknown => {}
            }
        }
    }

    // Stream ended without a `message_stop` event — server likely crashed or disconnected.
    Err(CoreError::StreamIncomplete)
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Anthropic Claude LLM provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl AnthropicProvider {
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
            .ok_or_else(|| CoreError::Llm("Anthropic API key not configured".to_string()))
    }

    async fn check_response(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, CoreError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

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

        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<AnthropicErrorResponse>(&body)
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
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        // Anthropic doesn't have a public list-models endpoint.
        // Return commonly available models.
        Ok(vec![
            "claude-sonnet-4-20250514".to_string(),
            "claude-opus-4-20250514".to_string(),
            "claude-haiku-3-5-20241022".to_string(),
        ])
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let url = format!("{}/messages", self.base_url());
        let api_key = self.api_key()?;
        let (system, messages) = convert_messages(&request.messages);
        let body = build_request_body(request, system, messages, false);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse response: {e}")))?;

        // Extract text, thinking, and tool calls from content blocks.
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut thinking_parts = Vec::new();

        for block in resp.content {
            match block {
                AnthropicResponseBlock::Text { text } => text_parts.push(text),
                AnthropicResponseBlock::Thinking { thinking } => thinking_parts.push(thinking),
                AnthropicResponseBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCallRequest {
                        id,
                        name,
                        arguments: serde_json::to_string(&input).unwrap_or_default(),
                        thought_signature: None,
                    });
                }
            }
        }

        let finish_reason = resp
            .stop_reason
            .as_deref()
            .map(parse_finish_reason)
            .unwrap_or(FinishReason::Other);

        let estimated_thinking = if !thinking_parts.is_empty() {
            let thinking_text = thinking_parts.join("");
            Some(estimate_tokens(&thinking_text))
        } else {
            None
        };

        let usage = resp
            .usage
            .map(|u| Usage {
                prompt_tokens: u.input_tokens,
                completion_tokens: u.output_tokens,
                total_tokens: u.input_tokens + u.output_tokens,
                thinking_tokens: estimated_thinking,
            })
            .unwrap_or_default();

        Ok(CompletionResponse {
            content: text_parts.join(""),
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            finish_reason,
            usage,
            thinking: if thinking_parts.is_empty() {
                None
            } else {
                Some(thinking_parts.join(""))
            },
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let url = format!("{}/messages", self.base_url());
        let api_key = self.api_key()?;
        let (system, messages) = convert_messages(&request.messages);
        let body = build_request_body(request, system, messages, true);

        info!("Anthropic stream request to {url}, model={}", request.model);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                error!("Anthropic stream send failed: {e}");
                if e.is_connect() || e.is_timeout() {
                    CoreError::TransientLlm(format!("Request failed: {e}"))
                } else {
                    CoreError::Llm(format!("Request failed: {e}"))
                }
            })?;

        info!("Anthropic stream response status: {}", response.status());
        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);
        info!("Anthropic SSE stream started");

        tokio::spawn(async move {
            if let Err(e) = parse_anthropic_stream(response, tx.clone()).await {
                error!("Anthropic SSE stream error: {e}");
                let _ = tx.send(Err(e)).await;
            }
            info!("Anthropic SSE stream ended");
        });

        let stream = futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        // Verify API key by making a minimal request.
        let url = format!("{}/messages", self.base_url());
        let api_key = self.api_key()?;

        let response = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": "claude-haiku-3-5-20241022",
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Health check failed: {e}")))?;

        self.check_response(response).await?;
        Ok(())
    }
}
