//! Anthropic Claude LLM provider.
//!
//! Implements the Anthropic Messages API which has a different format from
//! OpenAI: system prompts are top-level, tool schemas use `input_schema`,
//! and streaming uses named SSE events.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

use super::reasoning_profile::{resolve_reasoning_profile, ReasoningApiStyle};
use super::transport::{shared_http_transport, HttpTransport};
use super::{
    configured_request_timeout, next_stream_item_with_idle_timeout, send_stream_start_request,
    serialized_json_body, with_request_timeout, CompletionRequest, CompletionResponse, ContentPart,
    FinishReason, LlmProvider, Message, ProviderConfig, ReasoningEffort, Role, StreamChunk,
    ToolCallDelta, ToolCallRequest, ToolDefinition, Usage, DEFAULT_STREAM_IDLE_TIMEOUT,
};
use crate::conversation::memory::estimate_tokens;
use crate::error::CoreError;
use std::sync::Arc;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_TOKENS: u32 = 4096;
const MAX_CACHE_BREAKPOINTS: usize = 4;

// ---------------------------------------------------------------------------
// Anthropic API wire types — request
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicThinking {
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AnthropicOutputConfig {
    effort: String,
}

/// Anthropic `tool_choice`. We only emit `{"type":"auto", ...}` with an
/// explicit parallel-tool-use toggle; other variants (`any`/`tool`/`none`)
/// are not needed by any caller yet.
#[derive(Serialize)]
struct AnthropicToolChoice {
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_parallel_tool_use: Option<bool>,
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
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig>,
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
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Serialize)]
struct AnthropicImageSource {
    r#type: String,
    media_type: String,
    data: String,
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
        #[serde(default)]
        citations: Vec<AnthropicCitation>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    WebSearchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicCitation {
    WebSearchResultLocation {
        url: String,
        #[serde(default)]
        title: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
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
    RedactedThinking {
        data: String,
    },
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    WebSearchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum AnthropicStreamDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    CitationsDelta {
        citation: AnthropicCitation,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicDeltaUsage {
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
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

fn replayable_anthropic_thinking_blocks(
    message: &Message,
) -> Vec<super::provider_turn::AnthropicThinkingBlock> {
    if let Some(envelope) = message.provider_turn() {
        if let super::provider_turn::ProviderReplayPayload::AnthropicThinkingBlocks(blocks) =
            &envelope.replay_payload
        {
            return blocks.clone();
        }
    }
    message
        .tool_calls
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|call| call.thought_signature.as_deref())
        .find_map(super::provider_turn::decode_anthropic_thinking_blocks)
        .unwrap_or_default()
}

/// Convert our unified messages to Anthropic format.
///
/// - System messages are extracted and returned separately (Anthropic puts them top-level).
/// - Tool-result messages are wrapped in user messages with `tool_result` content blocks.
/// - Assistant messages with tool_calls become content block arrays.
fn convert_messages(
    messages: &[Message],
) -> (Option<Vec<AnthropicSystemBlock>>, Vec<AnthropicMessage>) {
    let mut system_blocks: Vec<AnthropicSystemBlock> = Vec::new();
    let mut out: Vec<AnthropicMessage> = Vec::new();
    for (index, msg) in messages.iter().enumerate() {
        let cache_boundary = msg
            .prompt_cache_hint()
            .is_some_and(|(stability, _)| stability != super::PromptStability::Volatile);
        match msg.role {
            Role::System => {
                let text = msg.text_content();
                if !text.is_empty() {
                    if messages
                        .iter()
                        .take(index)
                        .all(|message| message.role == Role::System)
                    {
                        system_blocks.push(AnthropicSystemBlock {
                            r#type: "text".to_string(),
                            text,
                            cache_control: cache_boundary.then(|| CacheControl {
                                r#type: "ephemeral".to_string(),
                            }),
                        });
                    } else {
                        let mut message = AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Text(text),
                        };
                        if cache_boundary {
                            add_cache_control_to_message_content(&mut message);
                        }
                        out.push(message);
                    }
                }
            }
            Role::User => {
                if msg.has_images() {
                    let blocks: Vec<AnthropicContentBlock> = msg
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => Some(AnthropicContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                            }),
                            ContentPart::Image { media_type, data } => {
                                Some(AnthropicContentBlock::Image {
                                    source: AnthropicImageSource {
                                        r#type: "base64".to_string(),
                                        media_type: media_type.clone(),
                                        data: data.clone(),
                                    },
                                })
                            }
                            ContentPart::ProviderTurn { .. } => None,
                        })
                        .collect();
                    let mut message = AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Blocks(blocks),
                    };
                    if cache_boundary {
                        add_cache_control_to_message_content(&mut message);
                    }
                    out.push(message);
                } else {
                    let mut message = AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Text(msg.text_content()),
                    };
                    if cache_boundary {
                        add_cache_control_to_message_content(&mut message);
                    }
                    out.push(message);
                }
            }
            Role::Assistant => {
                if let Some(ref calls) = msg.tool_calls {
                    // Build content blocks: text (if any) + tool_use blocks.
                    let mut blocks = Vec::new();
                    blocks.extend(replayable_anthropic_thinking_blocks(msg).into_iter().map(
                        |block| match block {
                            super::provider_turn::AnthropicThinkingBlock::Thinking {
                                thinking,
                                signature,
                            } => AnthropicContentBlock::Thinking {
                                thinking,
                                signature,
                            },
                            super::provider_turn::AnthropicThinkingBlock::RedactedThinking {
                                data,
                            } => AnthropicContentBlock::RedactedThinking { data },
                        },
                    ));
                    let text = msg.text_content();
                    if !text.is_empty() {
                        blocks.push(AnthropicContentBlock::Text {
                            text,
                            cache_control: None,
                        });
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
                                cache_control: None,
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
                                cache_control: None,
                            },
                        ]),
                    });
                }
            }
        }
    }

    let system = (!system_blocks.is_empty()).then_some(system_blocks);

    (system, out)
}

fn add_cache_control_to_message_content(message: &mut AnthropicMessage) {
    let cache_control = CacheControl {
        r#type: "ephemeral".to_string(),
    };
    match &mut message.content {
        AnthropicContent::Text(text) => {
            let text = std::mem::take(text);
            message.content = AnthropicContent::Blocks(vec![AnthropicContentBlock::Text {
                text,
                cache_control: Some(cache_control),
            }]);
        }
        AnthropicContent::Blocks(blocks) => {
            for block in blocks.iter_mut().rev() {
                match block {
                    AnthropicContentBlock::Text {
                        cache_control: target,
                        ..
                    }
                    | AnthropicContentBlock::ToolResult {
                        cache_control: target,
                        ..
                    } => {
                        *target = Some(cache_control.clone());
                        break;
                    }
                    AnthropicContentBlock::Image { .. }
                    | AnthropicContentBlock::Thinking { .. }
                    | AnthropicContentBlock::RedactedThinking { .. }
                    | AnthropicContentBlock::ToolUse { .. } => {}
                }
            }
        }
    }
}

fn convert_tools(tools: &[ToolDefinition], cache_tools: bool) -> Vec<serde_json::Value> {
    let has_native_search = tools
        .iter()
        .any(crate::llm::native_search::is_native_marker);
    let send_local_search = crate::llm::native_search::should_send_local_search(tools);
    let client_tools = tools
        .iter()
        .filter(|tool| !crate::llm::native_search::is_native_marker(tool))
        .filter(|tool| {
            send_local_search || tool.name != crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL
        })
        .collect::<Vec<_>>();
    let client_len = client_tools.len();
    let mut converted = client_tools
        .into_iter()
        .enumerate()
        .map(|(index, tool)| {
            let mut value = serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
            });
            // Anthropic caches everything up to the breakpoint. The built-in
            // search tool does not consume a client cache breakpoint.
            if cache_tools && index + 1 == client_len {
                value["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
            value
        })
        .collect::<Vec<_>>();
    if has_native_search {
        converted.push(serde_json::json!({
            "type": "web_search_20260209",
            "name": "web_search",
        }));
    }
    converted
}

fn consume_cache_breakpoint(cache_control: &mut Option<CacheControl>, remaining: &mut usize) {
    if cache_control.is_none() {
        return;
    }
    if *remaining == 0 {
        *cache_control = None;
    } else {
        *remaining -= 1;
    }
}

/// Anthropic counts tool, system, and message cache controls against one
/// request-wide limit. Keep the controls in wire-prefix order so the stable
/// tool/policy prefixes retain their breakpoints before replayable turns.
fn enforce_cache_breakpoint_limit(
    system: &mut Option<Vec<AnthropicSystemBlock>>,
    messages: &mut [AnthropicMessage],
    reserved_tool_breakpoints: usize,
) {
    let mut remaining = MAX_CACHE_BREAKPOINTS.saturating_sub(reserved_tool_breakpoints);

    if let Some(blocks) = system {
        for block in blocks {
            consume_cache_breakpoint(&mut block.cache_control, &mut remaining);
        }
    }

    for message in messages {
        let AnthropicContent::Blocks(blocks) = &mut message.content else {
            continue;
        };
        for block in blocks {
            match block {
                AnthropicContentBlock::Text { cache_control, .. }
                | AnthropicContentBlock::ToolResult { cache_control, .. } => {
                    consume_cache_breakpoint(cache_control, &mut remaining);
                }
                AnthropicContentBlock::Image { .. }
                | AnthropicContentBlock::Thinking { .. }
                | AnthropicContentBlock::RedactedThinking { .. }
                | AnthropicContentBlock::ToolUse { .. } => {}
            }
        }
    }
}

fn uses_adaptive_thinking(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "claude-fable-5" | "claude-mythos-5" | "claude-opus-4-8" | "claude-opus-4-7"
    )
}

fn anthropic_reasoning_effort(effort: Option<&ReasoningEffort>) -> Option<String> {
    match effort {
        Some(ReasoningEffort::None) => None,
        Some(ReasoningEffort::Minimal) | Some(ReasoningEffort::Low) => Some("low".to_string()),
        Some(ReasoningEffort::Medium) => Some("medium".to_string()),
        None => Some("high".to_string()),
        Some(ReasoningEffort::High) => Some("high".to_string()),
        Some(ReasoningEffort::XHigh) => Some("xhigh".to_string()),
        Some(ReasoningEffort::Max) => Some("max".to_string()),
    }
}

fn build_request_body(
    request: &CompletionRequest,
    mut system: Option<Vec<AnthropicSystemBlock>>,
    mut messages: Vec<AnthropicMessage>,
    stream: bool,
) -> AnthropicRequest {
    let supports_adaptive_thinking = uses_adaptive_thinking(&request.model);
    let uses_adaptive = supports_adaptive_thinking
        && (request.reasoning_effort.is_some() || request.thinking_budget.is_some());
    let temperature = if supports_adaptive_thinking {
        None
    } else {
        request.temperature
    };
    // NOTE: Anthropic's API returns a clear error for models that don't support
    // thinking, so budget-based thinking is not model-gated (unlike Gemini).
    let (thinking, output_config, temperature, effective_max_tokens) = if uses_adaptive {
        let effort = anthropic_reasoning_effort(request.reasoning_effort.as_ref());
        if let Some(effort) = effort {
            (
                Some(AnthropicThinking {
                    r#type: "adaptive".to_string(),
                    budget_tokens: None,
                }),
                Some(AnthropicOutputConfig { effort }),
                None,
                request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            )
        } else {
            (
                None,
                None,
                temperature,
                request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            )
        }
    } else if let Some(budget) = request.thinking_budget {
        let budget = budget.max(1024); // Anthropic requires budget_tokens >= 1024
        let base_max = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        // Ensure max_tokens > budget_tokens, with headroom for the response
        let effective_max = base_max.max(budget + 4096);
        (
            Some(AnthropicThinking {
                r#type: "enabled".to_string(),
                budget_tokens: Some(budget),
            }),
            None,
            None, // Anthropic requires temperature unset when thinking is enabled
            effective_max,
        )
    } else {
        (
            None,
            None,
            temperature,
            request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        )
    };

    let anthropic_tools = request.tools.as_ref().map(|t| convert_tools(t, true));
    let tool_cache_breakpoints = anthropic_tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|tool| tool.get("cache_control").is_some())
        .count();
    enforce_cache_breakpoint_limit(&mut system, &mut messages, tool_cache_breakpoints);
    let tool_choice = match anthropic_tools.as_ref() {
        Some(tools) if !tools.is_empty() && request.parallel_tool_calls => {
            Some(AnthropicToolChoice {
                r#type: "auto".to_string(),
                disable_parallel_tool_use: Some(false),
            })
        }
        _ => None,
    };

    AnthropicRequest {
        model: request.model.clone(),
        max_tokens: effective_max_tokens,
        system,
        messages,
        temperature,
        tools: anthropic_tools,
        tool_choice,
        stop_sequences: request.stop.clone(),
        stream: if stream { Some(true) } else { None },
        thinking,
        output_config,
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
fn anthropic_search_mode(request: &CompletionRequest) -> super::native_search::SearchExecutionMode {
    request
        .tools
        .as_deref()
        .and_then(super::native_search::marker_mode)
        .unwrap_or(super::native_search::SearchExecutionMode::NexaRouter)
}

fn drain_server_search_fallbacks(
    pending: &mut HashMap<String, serde_json::Value>,
    mode: super::native_search::SearchExecutionMode,
) -> Result<Vec<(String, serde_json::Value)>, CoreError> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    if !matches!(
        mode,
        super::native_search::SearchExecutionMode::Auto
            | super::native_search::SearchExecutionMode::Hybrid
    ) {
        return Err(CoreError::Llm(
            "Anthropic provider-native web search failed, and local search fallback is disabled for the selected search mode."
                .to_string(),
        ));
    }
    Ok(pending.drain().collect())
}

async fn parse_anthropic_stream(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, CoreError>>,
    search_mode: super::native_search::SearchExecutionMode,
) -> Result<(), CoreError> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    // Track input_tokens from message_start for final usage assembly.
    let mut input_tokens: u32 = 0;
    let mut cache_read_tokens: Option<u32> = None;
    let mut cache_creation_tokens: Option<u32> = None;
    // Track current tool call id/name from content_block_start.
    let mut current_tool_id = String::new();
    let mut current_tool_name: Option<String> = None;
    // Accumulate thinking content for token estimation.
    let mut thinking_text = String::new();
    let mut current_thinking_text = String::new();
    let mut current_thinking_signature = String::new();
    let mut current_block_is_thinking = false;
    let mut replay_thinking_blocks = Vec::new();
    let mut pending_server_searches = HashMap::<String, serde_json::Value>::new();
    let mut seen_citation_urls = HashSet::<String>::new();

    while let Some(chunk_result) = next_stream_item_with_idle_timeout(
        &mut byte_stream,
        DEFAULT_STREAM_IDLE_TIMEOUT,
        "Anthropic SSE stream",
    )
    .await?
    {
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
                        cache_read_tokens = u.cache_read_input_tokens;
                        cache_creation_tokens = u.cache_creation_input_tokens;
                    }
                }
                AnthropicStreamEvent::ContentBlockStart { content_block, .. } => {
                    match content_block {
                        AnthropicStreamContentBlock::Text { .. } => {
                            current_tool_id.clear();
                            current_tool_name = None;
                            current_block_is_thinking = false;
                        }
                        AnthropicStreamContentBlock::Thinking { .. } => {
                            current_tool_id.clear();
                            current_tool_name = None;
                            current_thinking_text.clear();
                            current_thinking_signature.clear();
                            current_block_is_thinking = true;
                        }
                        AnthropicStreamContentBlock::RedactedThinking { data } => {
                            current_tool_id.clear();
                            current_tool_name = None;
                            current_block_is_thinking = false;
                            if !data.trim().is_empty() {
                                replay_thinking_blocks.push(
                                    super::provider_turn::AnthropicThinkingBlock::RedactedThinking {
                                        data,
                                    },
                                );
                            }
                        }
                        AnthropicStreamContentBlock::ToolUse { id, name } => {
                            current_block_is_thinking = false;
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
                                    thought_signature:
                                        super::provider_turn::encode_anthropic_thinking_blocks(
                                            &replay_thinking_blocks,
                                        ),
                                }),
                                finish_reason: None,
                                usage: None,
                                thinking_delta: None,
                            };
                            if tx.send(Ok(chunk)).await.is_err() {
                                return Ok(());
                            }
                        }
                        AnthropicStreamContentBlock::ServerToolUse { id, name, input } => {
                            current_block_is_thinking = false;
                            current_tool_id.clear();
                            current_tool_name = None;
                            if name == super::native_search::LOCAL_WEB_SEARCH_TOOL {
                                pending_server_searches.insert(id, input);
                            }
                        }
                        AnthropicStreamContentBlock::WebSearchToolResult {
                            tool_use_id,
                            content,
                        } => {
                            current_block_is_thinking = false;
                            current_tool_id.clear();
                            current_tool_name = None;
                            // Array content is a completed provider search. An
                            // object is the documented in-band error shape, so
                            // keep the query pending for Nexa Router fallback.
                            if content.is_array() {
                                pending_server_searches.remove(&tool_use_id);
                            }
                        }
                        AnthropicStreamContentBlock::Unknown => {
                            current_block_is_thinking = false;
                            current_tool_id.clear();
                            current_tool_name = None;
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
                        current_thinking_text.push_str(&thinking);
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
                    AnthropicStreamDelta::SignatureDelta { signature } => {
                        current_thinking_signature.push_str(&signature);
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
                    AnthropicStreamDelta::CitationsDelta { citation } => {
                        if let AnthropicCitation::WebSearchResultLocation { url, title } = citation
                        {
                            if seen_citation_urls.insert(url.clone()) {
                                let label = title
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())
                                    .unwrap_or(&url);
                                let chunk = StreamChunk {
                                    delta: format!(" [{label}]({url})"),
                                    tool_call_delta: None,
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
                    AnthropicStreamDelta::Unknown => {}
                },
                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                    let mut finish = delta.stop_reason.as_deref().map(parse_finish_reason);
                    if delta.stop_reason.is_some() && !pending_server_searches.is_empty() {
                        for (id, input) in drain_server_search_fallbacks(
                            &mut pending_server_searches,
                            search_mode,
                        )? {
                            let chunk = StreamChunk {
                                delta: String::new(),
                                tool_call_delta: Some(ToolCallDelta {
                                    id,
                                    name: Some(
                                        super::native_search::LOCAL_WEB_SEARCH_TOOL.to_string(),
                                    ),
                                    arguments_delta: serde_json::to_string(&input)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                    index: None,
                                    thought_signature:
                                        super::provider_turn::encode_anthropic_thinking_blocks(
                                            &replay_thinking_blocks,
                                        ),
                                }),
                                finish_reason: None,
                                usage: None,
                                thinking_delta: None,
                            };
                            if tx.send(Ok(chunk)).await.is_err() {
                                return Ok(());
                            }
                        }
                        finish = Some(FinishReason::ToolCalls);
                    }
                    let estimated_thinking = if !thinking_text.is_empty() {
                        Some(estimate_tokens(&thinking_text))
                    } else {
                        None
                    };
                    let usage_info = usage.map(|u| {
                        if u.cache_read_input_tokens.is_some() {
                            cache_read_tokens = u.cache_read_input_tokens;
                        }
                        if u.cache_creation_input_tokens.is_some() {
                            cache_creation_tokens = u.cache_creation_input_tokens;
                        }
                        Usage {
                            prompt_tokens: input_tokens,
                            completion_tokens: u.output_tokens,
                            total_tokens: input_tokens + u.output_tokens,
                            thinking_tokens: estimated_thinking,
                            tool_prompt_tokens: None,
                            cache_read_tokens,
                            cache_miss_tokens: None,
                            cache_creation_tokens,
                            provider_raw: None,
                        }
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
                AnthropicStreamEvent::ContentBlockStop { .. } => {
                    if current_block_is_thinking && !current_thinking_signature.trim().is_empty() {
                        replay_thinking_blocks.push(
                            super::provider_turn::AnthropicThinkingBlock::Thinking {
                                thinking: current_thinking_text.clone(),
                                signature: current_thinking_signature.clone(),
                            },
                        );
                    }
                    current_block_is_thinking = false;
                }
                AnthropicStreamEvent::Ping | AnthropicStreamEvent::Unknown => {}
            }
        }
    }

    // Stream ended without a `message_stop` event — server likely crashed or disconnected.
    Err(CoreError::StreamIncomplete(
        "stream ended without message_stop event".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Anthropic Claude LLM provider.
pub struct AnthropicProvider {
    transport: Arc<HttpTransport>,
    config: ProviderConfig,
    request_timeout: Option<Duration>,
}

impl AnthropicProvider {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_native_search_uses_server_tool_without_duplicate_local_search() {
        let local = ToolDefinition {
            name: crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL.to_string(),
            description: "Local search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let file_tool = ToolDefinition {
            name: "read_file".to_string(),
            description: "Read file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let marker = crate::llm::native_search::NativeSearchPlan::resolve(
            crate::llm::native_search::SearchExecutionMode::ProviderNative,
            crate::llm::ProviderType::Anthropic,
            None,
            "claude-sonnet-5",
        )
        .marker()
        .unwrap();

        let tools = convert_tools(&[local, file_tool, marker], true);

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "read_file");
        assert_eq!(tools[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(tools[1]["type"], "web_search_20260209");
        assert_eq!(tools[1]["name"], "web_search");
    }

    #[test]
    fn provider_native_search_errors_do_not_fall_back_to_local_search() {
        let mut pending = HashMap::from([(
            "srvtoolu_1".to_string(),
            serde_json::json!({"query": "Nexa"}),
        )]);
        let error = drain_server_search_fallbacks(
            &mut pending,
            crate::llm::native_search::SearchExecutionMode::ProviderNative,
        )
        .expect_err("provider-native mode must fail closed");
        assert!(error
            .to_string()
            .contains("local search fallback is disabled"));
        assert_eq!(pending.len(), 1);

        let fallbacks = drain_server_search_fallbacks(
            &mut pending,
            crate::llm::native_search::SearchExecutionMode::Auto,
        )
        .expect("auto mode may use Nexa Router");
        assert_eq!(fallbacks.len(), 1);
        assert!(pending.is_empty());
    }

    #[test]
    fn signed_and_redacted_thinking_blocks_replay_in_provider_order() {
        let replay_blocks = vec![
            super::super::provider_turn::AnthropicThinkingBlock::Thinking {
                thinking: "private".to_string(),
                signature: "signed".to_string(),
            },
            super::super::provider_turn::AnthropicThinkingBlock::RedactedThinking {
                data: "opaque".to_string(),
            },
        ];
        let mut message = Message::text(Role::Assistant, "");
        message.tool_calls = Some(vec![ToolCallRequest {
            id: "toolu_1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: super::super::provider_turn::encode_anthropic_thinking_blocks(
                &replay_blocks,
            ),
        }]);

        let (_, messages) = convert_messages(&[message]);
        let value = serde_json::to_value(messages).unwrap();

        assert_eq!(value[0]["content"][0]["type"], "thinking");
        assert_eq!(value[0]["content"][0]["signature"], "signed");
        assert_eq!(value[0]["content"][1]["type"], "redacted_thinking");
        assert_eq!(value[0]["content"][1]["data"], "opaque");
        assert_eq!(value[0]["content"][2]["type"], "tool_use");
    }

    #[test]
    fn server_search_blocks_and_citations_are_forward_compatible() {
        let response: AnthropicResponse = serde_json::from_value(serde_json::json!({
            "content": [
                {
                    "type": "server_tool_use",
                    "id": "srvtoolu_1",
                    "name": "web_search",
                    "input": {"query": "Nexa"}
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_1",
                    "content": [{
                        "type": "web_search_result",
                        "url": "https://example.com/nexa",
                        "title": "Nexa"
                    }]
                },
                {
                    "type": "text",
                    "text": "Grounded answer",
                    "citations": [{
                        "type": "web_search_result_location",
                        "url": "https://example.com/nexa",
                        "title": "Nexa",
                        "cited_text": "Grounded answer",
                        "encrypted_index": "opaque"
                    }]
                }
            ],
            "stop_reason": "end_turn"
        }))
        .expect("server search response");

        assert_eq!(response.content.len(), 3);
        let AnthropicResponseBlock::Text { citations, .. } = &response.content[2] else {
            panic!("expected text block");
        };
        let evidence = crate::llm::native_search::SearchEvidence {
            dialect: crate::model_catalog::NativeSearchDialect::AnthropicServerTool,
            query: None,
            citations: citations
                .iter()
                .filter_map(|citation| match citation {
                    AnthropicCitation::WebSearchResultLocation { url, title } => {
                        Some(crate::llm::native_search::SearchCitation {
                            url: url.clone(),
                            title: title.clone(),
                            start_index: None,
                            end_index: None,
                        })
                    }
                    AnthropicCitation::Unknown => None,
                })
                .collect(),
        };
        assert!(
            crate::llm::native_search::render_citation_appendix(&evidence)
                .contains("[Nexa](https://example.com/nexa)")
        );
    }

    fn cacheable_message(
        role: Role,
        content: impl Into<String>,
        stability: crate::llm::PromptStability,
        boundary: crate::llm::CacheBoundaryHint,
    ) -> Message {
        Message::text(role, content).with_prompt_cache_hint(stability, boundary)
    }

    fn request_with_messages(
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> CompletionRequest {
        CompletionRequest {
            model: "claude-sonnet-4-5".to_string(),
            messages,
            temperature: Some(0.2),
            max_tokens: Some(1024),
            tools,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: None,
            routing_session_id: None,
            parallel_tool_calls: true,
        }
    }

    #[test]
    fn system_cache_control_stays_on_stable_first_block() {
        let messages = vec![
            cacheable_message(
                Role::System,
                "stable prompt",
                crate::llm::PromptStability::Stable,
                crate::llm::CacheBoundaryHint::PolicyEnd,
            ),
            Message::text(Role::System, "runtime date"),
            cacheable_message(
                Role::User,
                "hello",
                crate::llm::PromptStability::Replayable,
                crate::llm::CacheBoundaryHint::ReplayableTurnTail,
            ),
        ];

        let (system, api_messages) = convert_messages(&messages);
        let system_json = serde_json::to_value(system.unwrap()).unwrap();
        assert_eq!(
            system_json[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert!(system_json[1].get("cache_control").is_none());

        let messages_json = serde_json::to_value(api_messages).unwrap();
        assert_eq!(
            messages_json[0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn tool_cache_control_is_kept_when_system_has_volatile_blocks() {
        let tool = ToolDefinition {
            name: "search".into(),
            description: "Search".into(),
            parameters: serde_json::json!({"type":"object"}),
        };
        let messages = vec![
            cacheable_message(
                Role::System,
                "stable prompt",
                crate::llm::PromptStability::Stable,
                crate::llm::CacheBoundaryHint::PolicyEnd,
            ),
            Message::text(Role::System, "runtime plan"),
            cacheable_message(
                Role::User,
                "hello",
                crate::llm::PromptStability::Replayable,
                crate::llm::CacheBoundaryHint::ReplayableTurnTail,
            ),
        ];
        let (system, api_messages) = convert_messages(&messages);
        let body = build_request_body(
            &request_with_messages(messages, Some(vec![tool])),
            system,
            api_messages,
            false,
        );
        let body_json = serde_json::to_value(body).unwrap();
        assert_eq!(
            body_json["tools"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn cache_breakpoints_share_one_request_wide_limit_with_tools() {
        let tool = ToolDefinition {
            name: "search".into(),
            description: "Search".into(),
            parameters: serde_json::json!({"type":"object"}),
        };
        let messages = vec![
            cacheable_message(
                Role::System,
                "stable policy",
                crate::llm::PromptStability::Stable,
                crate::llm::CacheBoundaryHint::PolicyEnd,
            ),
            cacheable_message(
                Role::User,
                "turn one",
                crate::llm::PromptStability::Replayable,
                crate::llm::CacheBoundaryHint::ReplayableTurnTail,
            ),
            cacheable_message(
                Role::User,
                "turn two",
                crate::llm::PromptStability::Replayable,
                crate::llm::CacheBoundaryHint::ReplayableTurnTail,
            ),
            cacheable_message(
                Role::User,
                "turn three",
                crate::llm::PromptStability::Replayable,
                crate::llm::CacheBoundaryHint::ReplayableTurnTail,
            ),
        ];

        let (system, api_messages) = convert_messages(&messages);
        let body = build_request_body(
            &request_with_messages(messages, Some(vec![tool])),
            system,
            api_messages,
            false,
        );
        let body_json = serde_json::to_value(body).unwrap();

        fn count_cache_controls(value: &serde_json::Value) -> usize {
            match value {
                serde_json::Value::Array(values) => values.iter().map(count_cache_controls).sum(),
                serde_json::Value::Object(object) => {
                    usize::from(object.contains_key("cache_control"))
                        + object.values().map(count_cache_controls).sum::<usize>()
                }
                _ => 0,
            }
        }

        assert_eq!(count_cache_controls(&body_json), MAX_CACHE_BREAKPOINTS);
        assert_eq!(
            body_json["tools"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert!(body_json["messages"][2]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn non_leading_system_messages_are_user_context_not_top_level_system() {
        let messages = vec![
            Message::text(Role::System, "stable prompt"),
            Message::text(Role::User, "question"),
            Message::text(Role::System, "runtime tail"),
        ];

        let (system, api_messages) = convert_messages(&messages);
        let system_json = serde_json::to_value(system.unwrap()).unwrap();
        let messages_json = serde_json::to_value(api_messages).unwrap();

        assert_eq!(system_json.as_array().expect("system blocks").len(), 1);
        assert_eq!(system_json[0]["text"], "stable prompt");
        assert_eq!(messages_json[0]["role"], "user");
        assert_eq!(messages_json[0]["content"], "question");
        assert_eq!(messages_json[1]["role"], "user");
        assert_eq!(messages_json[1]["content"], "runtime tail");
    }

    #[test]
    fn user_cache_control_stays_on_original_user_not_tool_result() {
        let mut assistant = Message::text(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }]);
        let mut tool = Message::text(Role::Tool, "tool result");
        tool.name = Some("call-1".to_string());
        let messages = vec![
            Message::text(Role::System, "stable prompt"),
            cacheable_message(
                Role::User,
                "original request",
                crate::llm::PromptStability::Replayable,
                crate::llm::CacheBoundaryHint::ReplayableTurnTail,
            ),
            assistant,
            tool,
        ];

        let (_system, api_messages) = convert_messages(&messages);
        let messages_json = serde_json::to_value(api_messages).unwrap();

        assert_eq!(
            messages_json[0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert!(messages_json[2]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn opus_48_uses_adaptive_thinking_effort() {
        let messages = vec![Message::text(Role::User, "solve it")];
        let (_, api_messages) = convert_messages(&messages);
        let mut request = request_with_messages(messages, None);
        request.model = "claude-opus-4-8".to_string();
        request.reasoning_effort = Some(ReasoningEffort::XHigh);

        let body = build_request_body(&request, None, api_messages, false);
        let body_json = serde_json::to_value(body).unwrap();

        assert_eq!(
            body_json["thinking"],
            serde_json::json!({"type": "adaptive"})
        );
        assert_eq!(
            body_json["output_config"],
            serde_json::json!({"effort": "xhigh"})
        );
        assert!(body_json.get("temperature").is_none());
        assert!(body_json["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn fable_5_uses_adaptive_thinking_effort() {
        let messages = vec![Message::text(Role::User, "solve it")];
        let (_, api_messages) = convert_messages(&messages);
        let mut request = request_with_messages(messages, None);
        request.model = "claude-fable-5".to_string();
        request.reasoning_effort = Some(ReasoningEffort::Max);

        let body = build_request_body(&request, None, api_messages, false);
        let body_json = serde_json::to_value(body).unwrap();

        assert_eq!(
            body_json["thinking"],
            serde_json::json!({"type": "adaptive"})
        );
        assert_eq!(
            body_json["output_config"],
            serde_json::json!({"effort": "max"})
        );
        assert!(body_json.get("temperature").is_none());
        assert!(body_json["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn opus_48_prefers_adaptive_thinking_over_budget() {
        let messages = vec![Message::text(Role::User, "solve it")];
        let (_, api_messages) = convert_messages(&messages);
        let mut request = request_with_messages(messages, None);
        request.model = "claude-opus-4-8".to_string();
        request.thinking_budget = Some(10_000);

        let body = build_request_body(&request, None, api_messages, false);
        let body_json = serde_json::to_value(body).unwrap();

        assert_eq!(
            body_json["thinking"],
            serde_json::json!({"type": "adaptive"})
        );
        assert_eq!(
            body_json["output_config"],
            serde_json::json!({"effort": "high"})
        );
        assert!(body_json["thinking"].get("budget_tokens").is_none());
        assert_eq!(body_json["max_tokens"], 1024);
    }

    #[test]
    fn opus_48_omits_temperature_without_thinking() {
        let messages = vec![Message::text(Role::User, "answer directly")];
        let (_, api_messages) = convert_messages(&messages);
        let mut request = request_with_messages(messages, None);
        request.model = "claude-opus-4-8".to_string();

        let body = build_request_body(&request, None, api_messages, false);
        let body_json = serde_json::to_value(body).unwrap();

        assert!(body_json.get("temperature").is_none());
        assert!(body_json.get("thinking").is_none());
        assert!(body_json.get("output_config").is_none());
    }

    #[test]
    fn sonnet_45_uses_budget_thinking() {
        let messages = vec![Message::text(Role::User, "solve it")];
        let (_, api_messages) = convert_messages(&messages);
        let mut request = request_with_messages(messages, None);
        request.model = "claude-sonnet-4-5".to_string();
        request.thinking_budget = Some(2_000);

        let body = build_request_body(&request, None, api_messages, false);
        let body_json = serde_json::to_value(body).unwrap();

        assert_eq!(
            body_json["thinking"],
            serde_json::json!({"type": "enabled", "budget_tokens": 2000})
        );
        assert!(body_json.get("output_config").is_none());
        assert!(body_json.get("temperature").is_none());
        assert_eq!(body_json["max_tokens"], 6096);
    }

    #[test]
    fn anthropic_usage_deserializes_cache_tokens() {
        let usage: AnthropicUsage = serde_json::from_value(serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 25,
            "cache_read_input_tokens": 80,
            "cache_creation_input_tokens": 10
        }))
        .unwrap();

        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.cache_creation_input_tokens, Some(10));
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn prompt_cache_profile(&self, model: &str) -> super::prompt_cache::PromptCacheProfile {
        super::prompt_cache::resolve_prompt_cache_profile(
            self.config.provider_type,
            self.config.base_url.as_deref(),
            super::prompt_cache::PromptCacheApiStyle::AnthropicMessages,
            model,
        )
    }

    fn reasoning_replay_policy(
        &self,
        model: &str,
    ) -> super::reasoning_profile::ReasoningReplayPolicy {
        resolve_reasoning_profile(
            self.config.provider_type,
            self.config.base_url.as_deref(),
            ReasoningApiStyle::AnthropicMessages,
            model,
        )
        .replay_policy
    }

    fn route_snapshot(&self, request: &CompletionRequest) -> super::provider_turn::RouteSnapshot {
        let profile = resolve_reasoning_profile(
            self.config.provider_type,
            self.config.base_url.as_deref(),
            ReasoningApiStyle::AnthropicMessages,
            &request.model,
        );
        let mut snapshot =
            super::provider_turn::RouteSnapshot::from_profile_for_request(&profile, request);
        if request.reasoning_enabled != Some(true)
            && request.reasoning_effort.is_none()
            && request.thinking_budget.is_none()
        {
            snapshot.replay_policy = super::reasoning_profile::ReasoningReplayPolicy::NotRequired;
        }
        snapshot
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        // Anthropic doesn't have a public list-models endpoint.
        // Return commonly available models.
        Ok(vec![
            "claude-fable-5".to_string(),
            "claude-mythos-5".to_string(),
            "claude-opus-4-8".to_string(),
            "claude-opus-4-7".to_string(),
            "claude-opus-4-6".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-opus-4-5".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-haiku-4-5".to_string(),
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
        let body_bytes = serialized_json_body(&body, "Anthropic completion request")?;

        let response = with_request_timeout(
            self.transport
                .client()
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("anthropic-beta", "prompt-caching-2024-07-31")
                .header("Content-Type", "application/json")
                .body(body_bytes),
            self.request_timeout,
        )
        .send()
        .await
        .inspect_err(|error| {
            self.transport.record_transport_failure(&error.to_string());
        })
        .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: AnthropicResponse = response
            .json()
            .await
            .inspect_err(|error| {
                self.transport.record_transport_failure(&error.to_string());
            })
            .map_err(|e| CoreError::Llm(format!("Failed to parse response: {e}")))?;
        self.transport.record_transport_success();

        // Extract text, thinking, and tool calls from content blocks.
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut thinking_parts = Vec::new();
        let mut thinking_blocks = Vec::new();
        let mut pending_server_searches = HashMap::<String, serde_json::Value>::new();
        let mut completed_server_searches = HashSet::<String>::new();
        let mut citations = Vec::new();
        let search_mode = anthropic_search_mode(request);

        for block in resp.content {
            match block {
                AnthropicResponseBlock::Text {
                    text,
                    citations: block_citations,
                } => {
                    text_parts.push(text);
                    citations.extend(block_citations);
                }
                AnthropicResponseBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    if let Some(signature) = signature.filter(|value| !value.trim().is_empty()) {
                        thinking_blocks.push(
                            super::provider_turn::AnthropicThinkingBlock::Thinking {
                                thinking: thinking.clone(),
                                signature,
                            },
                        );
                    }
                    thinking_parts.push(thinking);
                }
                AnthropicResponseBlock::RedactedThinking { data } => {
                    if !data.trim().is_empty() {
                        thinking_blocks.push(
                            super::provider_turn::AnthropicThinkingBlock::RedactedThinking { data },
                        );
                    }
                }
                AnthropicResponseBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCallRequest {
                        id,
                        name,
                        arguments: serde_json::to_string(&input).unwrap_or_default(),
                        thought_signature: None,
                    });
                }
                AnthropicResponseBlock::ServerToolUse { id, name, input } => {
                    if name == super::native_search::LOCAL_WEB_SEARCH_TOOL {
                        pending_server_searches.insert(id, input);
                    }
                }
                AnthropicResponseBlock::WebSearchToolResult {
                    tool_use_id,
                    content,
                } => {
                    if content.is_array() {
                        completed_server_searches.insert(tool_use_id);
                    }
                }
                AnthropicResponseBlock::Unknown => {}
            }
        }

        for completed in completed_server_searches {
            pending_server_searches.remove(&completed);
        }
        if resp.stop_reason.is_some() {
            tool_calls.extend(
                drain_server_search_fallbacks(&mut pending_server_searches, search_mode)?
                    .into_iter()
                    .map(|(id, input)| ToolCallRequest {
                        id,
                        name: super::native_search::LOCAL_WEB_SEARCH_TOOL.to_string(),
                        arguments: serde_json::to_string(&input)
                            .unwrap_or_else(|_| "{}".to_string()),
                        thought_signature: None,
                    }),
            );
        }
        if let Some(first_tool_call) = tool_calls.first_mut() {
            first_tool_call.thought_signature =
                super::provider_turn::encode_anthropic_thinking_blocks(&thinking_blocks);
        }

        let evidence = super::native_search::SearchEvidence {
            dialect: crate::model_catalog::NativeSearchDialect::AnthropicServerTool,
            query: None,
            citations: citations
                .into_iter()
                .filter_map(|citation| match citation {
                    AnthropicCitation::WebSearchResultLocation { url, title } => {
                        Some(super::native_search::SearchCitation {
                            url,
                            title,
                            start_index: None,
                            end_index: None,
                        })
                    }
                    AnthropicCitation::Unknown => None,
                })
                .collect(),
        };
        let citation_appendix = super::native_search::render_citation_appendix(&evidence);

        let finish_reason = if !tool_calls.is_empty() {
            FinishReason::ToolCalls
        } else {
            resp.stop_reason
                .as_deref()
                .map(parse_finish_reason)
                .unwrap_or(FinishReason::Other)
        };

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
                tool_prompt_tokens: None,
                cache_read_tokens: u.cache_read_input_tokens,
                cache_miss_tokens: None,
                cache_creation_tokens: u.cache_creation_input_tokens,
                provider_raw: None,
            })
            .unwrap_or_default();

        Ok(CompletionResponse {
            content: format!("{}{}", text_parts.join(""), citation_appendix),
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
        let body_bytes = serialized_json_body(&body, "Anthropic stream request")?;

        info!("Anthropic stream request to {url}, model={}", request.model);

        let response = send_stream_start_request(
            self.transport
                .client()
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("anthropic-beta", "prompt-caching-2024-07-31")
                .header("Content-Type", "application/json")
                .body(body_bytes),
            self.request_timeout,
            "Anthropic stream request",
        )
        .await
        .inspect_err(|e| {
            self.transport.record_transport_failure(&e.to_string());
            error!("Anthropic stream send failed: {e}");
        })?;

        info!("Anthropic stream response status: {}", response.status());
        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);
        info!("Anthropic SSE stream started");

        let transport = Arc::clone(&self.transport);
        let search_mode = anthropic_search_mode(request);
        tokio::spawn(async move {
            if let Err(e) = parse_anthropic_stream(response, tx.clone(), search_mode).await {
                transport.record_transport_failure(&e.to_string());
                error!("Anthropic SSE stream error: {e}");
                let _ = tx.send(Err(e)).await;
            } else {
                transport.record_transport_success();
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

        let response = with_request_timeout(
            self.transport
                .client()
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "model": "claude-haiku-3-5-20241022",
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "hi"}]
                })),
            self.request_timeout,
        )
        .send()
        .await
        .map_err(|e| CoreError::Llm(format!("Health check failed: {e}")))?;

        self.check_response(response).await?;
        Ok(())
    }
}
