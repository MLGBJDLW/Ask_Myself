//! Google Gemini LLM provider.
//!
//! Uses the Gemini REST API with API key authentication via query parameter.
//! System prompts use top-level `systemInstruction`, roles map "assistant" → "model",
//! and tool calls use `functionCall`/`functionResponse` parts.

use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{error, info};

use super::{
    configured_request_timeout, next_stream_item_with_idle_timeout, send_stream_start_request,
    with_request_timeout, CompletionRequest, CompletionResponse, ContentPart, FinishReason,
    LlmProvider, Message, ProviderConfig, Role, StreamChunk, ToolCallDelta, ToolCallRequest,
    ToolDefinition, Usage, DEFAULT_STREAM_IDLE_TIMEOUT,
};
use crate::error::CoreError;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_TIMEOUT_SECS: u64 = 300;

// Gemini can emit relatively large SSE payloads, especially for thought parts.
// Splitting only the provider-local delta preserves protocol semantics while
// making the UI feel like a token stream instead of paragraph-sized bursts.
const GEMINI_TEXT_CHUNK_CHARS: usize = 24;
const GEMINI_THINKING_CHUNK_CHARS: usize = 32;
const GEMINI_MICRO_CHUNK_DELAY: Duration = Duration::from_millis(12);

// ---------------------------------------------------------------------------
// Gemini API wire types
// ---------------------------------------------------------------------------

/// A part in a Gemini content message. Uses untagged enum for correct JSON layout.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
enum GeminiPartV2 {
    // Thought must come BEFORE Text for serde untagged matching:
    // {"text":"…","thought":true} matches Thought first, {"text":"…"} falls through to Text.
    Thought {
        text: String,
        thought: bool,
    },
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
        #[serde(
            rename = "thoughtSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        thought_signature: Option<String>,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: GeminiBlob,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiBlob {
    mime_type: String,
    data: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContentV2 {
    role: String,
    parts: Vec<GeminiPartV2>,
}

#[derive(Serialize)]
struct GeminiSystemInstructionV2 {
    parts: Vec<GeminiPartV2>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequestV2 {
    contents: Vec<GeminiContentV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstructionV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolSet>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolSet {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_thoughts: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
}

// ---------------------------------------------------------------------------
// Gemini API wire types — response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    parts: Option<Vec<GeminiPartV2>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
    #[serde(default)]
    thoughts_token_count: Option<i64>,
}

#[derive(Deserialize)]
struct GeminiErrorResponse {
    error: GeminiErrorBody,
}

#[derive(Deserialize)]
struct GeminiErrorBody {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiListModelsResponse {
    models: Option<Vec<GeminiModel>>,
}

#[derive(Deserialize)]
struct GeminiModel {
    name: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" => FinishReason::ContentFilter,
        "RECITATION" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

/// Convert unified messages to Gemini format.
/// Only the leading system prefix is extracted as top-level systemInstruction.
fn convert_messages(
    messages: &[Message],
) -> (Option<GeminiSystemInstructionV2>, Vec<GeminiContentV2>) {
    let mut system_parts: Vec<GeminiPartV2> = Vec::new();
    let mut contents: Vec<GeminiContentV2> = Vec::new();
    let mut tool_id_to_name: HashMap<String, String> = HashMap::new();

    for (index, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::System => {
                let text = msg.text_content();
                if messages
                    .iter()
                    .take(index)
                    .all(|message| message.role == Role::System)
                {
                    system_parts.push(GeminiPartV2::Text { text });
                } else if !text.is_empty() {
                    contents.push(GeminiContentV2 {
                        role: "user".to_string(),
                        parts: vec![GeminiPartV2::Text { text }],
                    });
                }
            }
            Role::User => {
                let parts: Vec<GeminiPartV2> = msg
                    .parts
                    .iter()
                    .map(|p| match p {
                        ContentPart::Text { text } => GeminiPartV2::Text { text: text.clone() },
                        ContentPart::Image { media_type, data } => GeminiPartV2::InlineData {
                            inline_data: GeminiBlob {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            },
                        },
                    })
                    .collect();
                contents.push(GeminiContentV2 {
                    role: "user".to_string(),
                    parts,
                });
            }
            Role::Assistant => {
                let mut parts: Vec<GeminiPartV2> = Vec::new();
                let text = msg.text_content();
                if !text.is_empty() {
                    parts.push(GeminiPartV2::Text { text });
                }
                if let Some(ref calls) = msg.tool_calls {
                    for tc in calls {
                        tool_id_to_name.insert(tc.id.clone(), tc.name.clone());
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                        parts.push(GeminiPartV2::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: tc.name.clone(),
                                args,
                            },
                            thought_signature: tc.thought_signature.clone(),
                        });
                    }
                }
                contents.push(GeminiContentV2 {
                    role: "model".to_string(),
                    parts,
                });
            }
            Role::Tool => {
                // Gemini expects function responses as user-role parts.
                let tool_ref = msg.name.clone().unwrap_or_default();
                let tool_name = tool_id_to_name.get(&tool_ref).cloned().unwrap_or(tool_ref);

                // Gemini requires an object-like payload for functionResponse.response.
                let text = msg.text_content();
                let mut response_val: serde_json::Value = serde_json::from_str(&text)
                    .unwrap_or_else(|_| serde_json::json!({ "content": text }));
                if !response_val.is_object() {
                    response_val = serde_json::json!({ "content": response_val });
                }

                let make_part = || GeminiPartV2::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: tool_name.clone(),
                        response: response_val.clone(),
                    },
                };

                // Append to the last user content if possible, otherwise new user message.
                let appended = if let Some(last) = contents.last_mut() {
                    if last.role == "user" {
                        last.parts.push(make_part());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !appended {
                    contents.push(GeminiContentV2 {
                        role: "user".to_string(),
                        parts: vec![make_part()],
                    });
                }
            }
        }
    }

    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(GeminiSystemInstructionV2 {
            parts: system_parts,
        })
    };

    (system_instruction, contents)
}

/// Recursively removes JSON Schema fields that Google Gemini API does not accept.
fn clean_schema_for_gemini(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let cleaned: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter(|(key, _)| {
                    key.as_str() != "$schema" && key.as_str() != "additionalProperties"
                })
                .map(|(key, val)| (key.clone(), clean_schema_for_gemini(val)))
                .collect();
            serde_json::Value::Object(cleaned)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(clean_schema_for_gemini).collect())
        }
        other => other.clone(),
    }
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<GeminiToolSet> {
    vec![GeminiToolSet {
        function_declarations: tools
            .iter()
            .map(|t| GeminiFunctionDeclaration {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: clean_schema_for_gemini(&t.parameters),
            })
            .collect(),
    }]
}

/// Returns `true` if the model supports extended thinking (`thinking_config`).
/// Currently only Gemini 2.5+ models support it.
fn supports_thinking(model: &str) -> bool {
    // Normalise: "models/gemini-2.5-flash" → "gemini-2.5-flash"
    let name = model
        .strip_prefix("models/")
        .unwrap_or(model)
        .to_lowercase();

    // Quick check: contains "2.5" (covers gemini-2.5-flash, gemini-2.5-pro, previews, etc.)
    if name.contains("2.5") {
        return true;
    }

    // Forward-compat: any gemini-<major>. where major >= 3
    if let Some(rest) = name.strip_prefix("gemini-") {
        if let Some(major_str) = rest.split('.').next() {
            if let Ok(major) = major_str.parse::<u32>() {
                return major >= 3;
            }
        }
    }

    false
}

fn build_request_body(
    request: &CompletionRequest,
    system_instruction: Option<GeminiSystemInstructionV2>,
    contents: Vec<GeminiContentV2>,
) -> GeminiRequestV2 {
    // Only send thinking_config to models that support it (Gemini 2.5+).
    let thinking_config = if supports_thinking(&request.model) {
        request.thinking_budget.map(|budget| {
            // Gemini API requires budget in 128..=32768. Clamp to avoid API errors.
            let clamped = budget.clamp(128, 32_768) as i32;
            GeminiThinkingConfig {
                thinking_budget: Some(clamped),
                // Required to receive `thought: true` parts in streaming/non-streaming responses.
                include_thoughts: Some(true),
            }
        })
    } else {
        None
    };
    let has_thinking = thinking_config.is_some();

    let generation_config = if request.temperature.is_some()
        || request.max_tokens.is_some()
        || request.stop.is_some()
        || thinking_config.is_some()
    {
        Some(GeminiGenerationConfig {
            // Gemini requires temperature unset when thinking is enabled.
            temperature: if has_thinking {
                None
            } else {
                request.temperature
            },
            max_output_tokens: request.max_tokens,
            stop_sequences: request.stop.clone(),
            thinking_config,
        })
    } else {
        None
    };

    GeminiRequestV2 {
        contents,
        system_instruction,
        tools: request.tools.as_ref().map(|t| convert_tools(t)),
        generation_config,
    }
}

/// Extract text, tool calls, finish reason, and usage from a Gemini response.
fn extract_response(
    resp: &GeminiResponse,
) -> (
    String,
    Vec<ToolCallRequest>,
    FinishReason,
    Usage,
    Option<String>,
) {
    let candidate = resp.candidates.as_ref().and_then(|c| c.first());

    let mut text_parts = Vec::new();
    let mut thinking_parts = Vec::new();
    let mut tool_calls = Vec::new();

    if let Some(candidate) = candidate {
        if let Some(ref content) = candidate.content {
            if let Some(ref parts) = content.parts {
                for (idx, part) in parts.iter().enumerate() {
                    match part {
                        GeminiPartV2::Thought { text, thought } if *thought => {
                            thinking_parts.push(text.clone());
                        }
                        GeminiPartV2::Thought { text, .. } | GeminiPartV2::Text { text } => {
                            text_parts.push(text.clone());
                        }
                        GeminiPartV2::FunctionCall {
                            function_call,
                            thought_signature,
                        } => {
                            tool_calls.push(ToolCallRequest {
                                id: format!("call_{idx}"),
                                name: function_call.name.clone(),
                                arguments: serde_json::to_string(&function_call.args)
                                    .unwrap_or_default(),
                                thought_signature: thought_signature.clone(),
                            });
                        }
                        GeminiPartV2::FunctionResponse { .. } | GeminiPartV2::InlineData { .. } => {
                        }
                    }
                }
            }
        }
    }

    let finish_reason = candidate
        .and_then(|c| c.finish_reason.as_deref())
        .map(parse_finish_reason)
        .unwrap_or(if tool_calls.is_empty() {
            FinishReason::Other
        } else {
            FinishReason::ToolCalls
        });

    let usage = resp
        .usage_metadata
        .as_ref()
        .map(|u| Usage {
            prompt_tokens: u.prompt_token_count.unwrap_or(0),
            completion_tokens: u.candidates_token_count.unwrap_or(0),
            total_tokens: u.total_token_count.unwrap_or(0),
            thinking_tokens: u.thoughts_token_count.map(|t| t.max(0) as u32),
            cache_read_tokens: None,
            cache_miss_tokens: None,
            cache_creation_tokens: None,
        })
        .unwrap_or_default();

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join(""))
    };

    (
        text_parts.join(""),
        tool_calls,
        finish_reason,
        usage,
        thinking,
    )
}

/// Convert provider chunk content to incremental deltas.
///
/// Some Gemini stream chunks are cumulative while others are already delta-like.
/// This helper emits only the new suffix when cumulative text is detected.
fn to_incremental_delta(previous: &mut String, current: String) -> String {
    if current.is_empty() {
        return String::new();
    }
    let delta = if current.starts_with(previous.as_str()) {
        current[previous.len()..].to_string()
    } else {
        current.clone()
    };
    *previous = current;
    delta
}

// ---------------------------------------------------------------------------
// Gemini SSE stream parser
// ---------------------------------------------------------------------------

/// Append one arbitrary network byte chunk to a UTF-8 string buffer.
///
/// `reqwest::bytes_stream()` does not guarantee that each yielded byte chunk is
/// a complete UTF-8 string. Large Chinese/Japanese/Korean text or emoji can be
/// split across byte chunks, so per-chunk `str::from_utf8` creates false
/// "incomplete utf-8 byte sequence" failures. This helper keeps an unfinished
/// UTF-8 suffix in `pending_utf8` until the next network chunk arrives.
fn push_utf8_stream_chunk(
    buffer: &mut String,
    pending_utf8: &mut Vec<u8>,
    chunk: &[u8],
) -> Result<(), CoreError> {
    pending_utf8.extend_from_slice(chunk);

    loop {
        match std::str::from_utf8(pending_utf8) {
            Ok(text) => {
                buffer.push_str(text);
                pending_utf8.clear();
                return Ok(());
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    let valid = std::str::from_utf8(&pending_utf8[..valid_up_to])
                        .map_err(|e| CoreError::Llm(format!("Invalid UTF-8 in stream: {e}")))?
                        .to_string();
                    buffer.push_str(&valid);
                    pending_utf8.drain(..valid_up_to);
                    continue;
                }

                // No valid prefix and no definite invalid byte: the entire
                // pending buffer is an incomplete multi-byte sequence. Wait for
                // the next network chunk instead of failing the stream.
                if error.error_len().is_none() {
                    return Ok(());
                }

                return Err(CoreError::Llm(format!("Invalid UTF-8 in stream: {error}")));
            }
        }
    }
}

fn split_delta_for_streaming(delta: &str, target_chars: usize) -> Vec<String> {
    if delta.is_empty() {
        return Vec::new();
    }

    let target_chars = target_chars.max(1);
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;

    for ch in delta.chars() {
        current.push(ch);
        current_chars += 1;

        let soft_boundary = matches!(
            ch,
            '\n' | '\r'
                | '。'
                | '！'
                | '？'
                | '，'
                | '、'
                | '；'
                | '：'
                | '.'
                | '!'
                | '?'
                | ','
                | ';'
                | ':'
        );

        if current_chars >= target_chars || (current_chars >= 8 && soft_boundary) {
            parts.push(std::mem::take(&mut current));
            current_chars = 0;
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

async fn send_gemini_content_chunks(
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    text_delta: String,
    thinking_delta: Option<String>,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
) -> bool {
    let text_parts = split_delta_for_streaming(&text_delta, GEMINI_TEXT_CHUNK_CHARS);
    let thinking_parts = thinking_delta
        .as_deref()
        .map(|delta| split_delta_for_streaming(delta, GEMINI_THINKING_CHUNK_CHARS))
        .unwrap_or_default();
    let needs_terminal_chunk = finish_reason.is_some() || usage.is_some();
    let chunk_count = text_parts
        .len()
        .max(thinking_parts.len())
        .max(if needs_terminal_chunk { 1 } else { 0 });

    for idx in 0..chunk_count {
        let is_last = idx + 1 == chunk_count;
        let chunk = StreamChunk {
            delta: text_parts.get(idx).cloned().unwrap_or_default(),
            tool_call_delta: None,
            finish_reason: if is_last { finish_reason.clone() } else { None },
            usage: if is_last { usage.clone() } else { None },
            thinking_delta: thinking_parts.get(idx).cloned(),
        };

        let has_payload = !chunk.delta.is_empty()
            || chunk
                .thinking_delta
                .as_deref()
                .is_some_and(|delta| !delta.is_empty())
            || chunk.finish_reason.is_some()
            || chunk.usage.is_some();
        if !has_payload {
            continue;
        }

        if tx.send(Ok(chunk)).await.is_err() {
            return false;
        }

        if !is_last && !GEMINI_MICRO_CHUNK_DELAY.is_zero() {
            tokio::time::sleep(GEMINI_MICRO_CHUNK_DELAY).await;
        }
    }

    true
}

async fn emit_gemini_response_chunk(
    resp: GeminiResponse,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    emitted_text: &mut String,
    emitted_thinking: &mut String,
    saw_finish_reason: &mut bool,
) -> bool {
    let (text_content, tool_calls, finish_reason, usage, thinking) = extract_response(&resp);
    let text_delta = to_incremental_delta(emitted_text, text_content);
    let thinking_delta = thinking
        .map(|t| to_incremental_delta(emitted_thinking, t))
        .filter(|s| !s.is_empty());
    let has_finish = finish_reason != FinishReason::Other;
    if has_finish {
        *saw_finish_reason = true;
    }

    if !text_delta.is_empty() || has_finish || usage.total_tokens > 0 || thinking_delta.is_some() {
        let finish_reason = if has_finish {
            Some(finish_reason)
        } else {
            None
        };
        let usage = if usage.total_tokens > 0 {
            Some(usage)
        } else {
            None
        };
        if !send_gemini_content_chunks(tx, text_delta, thinking_delta, finish_reason, usage).await {
            return false;
        }
    }

    for tc in &tool_calls {
        let delta_chunk = StreamChunk {
            delta: String::new(),
            tool_call_delta: Some(ToolCallDelta {
                id: tc.id.clone(),
                name: Some(tc.name.clone()),
                arguments_delta: tc.arguments.clone(),
                index: tc
                    .id
                    .strip_prefix("call_")
                    .and_then(|s| s.parse::<u32>().ok()),
                thought_signature: tc.thought_signature.clone(),
            }),
            finish_reason: None,
            usage: None,
            thinking_delta: None,
        };
        if tx.send(Ok(delta_chunk)).await.is_err() {
            return false;
        }
    }

    true
}

async fn process_gemini_sse_event(
    data: &str,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    emitted_text: &mut String,
    emitted_thinking: &mut String,
    saw_finish_reason: &mut bool,
) -> Result<bool, CoreError> {
    let data = data.trim();
    if data.is_empty() {
        return Ok(true);
    }
    if data == "[DONE]" {
        return Ok(false);
    }

    let resp: GeminiResponse = match serde_json::from_str(data) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("Gemini SSE parse skip: {e}");
            return Ok(true);
        }
    };

    Ok(
        emit_gemini_response_chunk(resp, tx, emitted_text, emitted_thinking, saw_finish_reason)
            .await,
    )
}

/// Parse Gemini's SSE streaming response.
///
/// Gemini streams the same JSON response shape as non-streaming, one chunk per SSE event.
async fn parse_gemini_stream(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, CoreError>>,
) -> Result<(), CoreError> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut pending_utf8: Vec<u8> = Vec::new();
    let mut event_data_lines: Vec<String> = Vec::new();
    let mut emitted_text = String::new();
    let mut emitted_thinking = String::new();
    let mut saw_finish_reason = false;

    while let Some(chunk_result) = next_stream_item_with_idle_timeout(
        &mut byte_stream,
        DEFAULT_STREAM_IDLE_TIMEOUT,
        "Gemini SSE stream",
    )
    .await?
    {
        let chunk = chunk_result.map_err(|e| CoreError::Llm(format!("Stream read error: {e}")))?;
        push_utf8_stream_chunk(&mut buffer, &mut pending_utf8, &chunk)?;

        // Process complete lines.
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            // Empty line marks the end of an SSE event; process buffered `data:` lines.
            if line.is_empty() {
                if event_data_lines.is_empty() {
                    continue;
                }

                let data = event_data_lines.join("\n");
                event_data_lines.clear();
                if !process_gemini_sse_event(
                    &data,
                    &tx,
                    &mut emitted_text,
                    &mut emitted_thinking,
                    &mut saw_finish_reason,
                )
                .await?
                {
                    return Ok(());
                }
                continue;
            }

            // Accumulate `data:` lines (Gemini may split one JSON event across multiple lines).
            if let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            {
                event_data_lines.push(data.to_string());
            }
        }
    }

    if !pending_utf8.is_empty() {
        return Err(CoreError::Llm(
            "Invalid UTF-8 in stream: incomplete UTF-8 byte sequence at end of stream".to_string(),
        ));
    }

    // Flush a trailing event if the stream ended without a blank line.
    if !event_data_lines.is_empty() {
        let data = event_data_lines.join("\n");
        let _ = process_gemini_sse_event(
            &data,
            &tx,
            &mut emitted_text,
            &mut emitted_thinking,
            &mut saw_finish_reason,
        )
        .await?;
    }

    if saw_finish_reason {
        Ok(())
    } else {
        // Stream ended without a finishReason — server likely crashed or disconnected.
        Err(CoreError::StreamIncomplete(
            "stream ended without finishReason".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Google Gemini LLM provider.
pub struct GeminiProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    request_timeout: Option<Duration>,
}

impl GeminiProvider {
    pub fn new(config: ProviderConfig) -> Result<Self, CoreError> {
        let timeout = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        let request_timeout = configured_request_timeout(timeout);
        // Force HTTP/1.1 + short idle pool TTL so SSE streams are not
        // interrupted by upstream HTTP/2 RST_STREAM frames on stale sockets.
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
            .ok_or_else(|| CoreError::Llm("Google API key not configured".to_string()))
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
        let message = serde_json::from_str::<GeminiErrorResponse>(&body)
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
impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        let api_key = self.api_key()?;
        let url = format!("{}/models?key={}", self.base_url(), api_key);

        let response = with_request_timeout(self.client.get(&url), self.request_timeout)
            .send()
            .await
            .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: GeminiListModelsResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse models response: {e}")))?;

        Ok(resp
            .models
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                // Gemini returns "models/gemini-pro" — strip the prefix.
                m.name
                    .strip_prefix("models/")
                    .unwrap_or(&m.name)
                    .to_string()
            })
            .collect())
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let api_key = self.api_key()?;
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url(),
            request.model,
            api_key,
        );

        let (system_instruction, contents) = convert_messages(&request.messages);
        let body = build_request_body(request, system_instruction, contents);

        let response = with_request_timeout(
            self.client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&body),
            self.request_timeout,
        )
        .send()
        .await
        .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;

        let response = self.check_response(response).await?;

        let resp: GeminiResponse = response
            .json()
            .await
            .map_err(|e| CoreError::Llm(format!("Failed to parse response: {e}")))?;

        let (content, tool_calls, finish_reason, usage, thinking) = extract_response(&resp);

        Ok(CompletionResponse {
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            finish_reason,
            usage,
            thinking,
        })
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
    ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
        let api_key = self.api_key()?;
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url(),
            request.model,
            api_key,
        );

        let (system_instruction, contents) = convert_messages(&request.messages);
        let body = build_request_body(request, system_instruction, contents);

        info!(
            "Gemini stream request to {}..., model={}",
            &url[..url.find("key=").unwrap_or(url.len())],
            request.model
        );

        let response = send_stream_start_request(
            self.client
                .post(&url)
                .header("Content-Type", "application/json")
                .json(&body),
            self.request_timeout,
            "Gemini stream request",
        )
        .await
        .map_err(|e| {
            error!("Gemini stream send failed: {e}");
            e
        })?;

        info!("Gemini stream response status: {}", response.status());
        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);
        info!("Gemini SSE stream started");

        tokio::spawn(async move {
            if let Err(e) = parse_gemini_stream(response, tx.clone()).await {
                error!("Gemini SSE stream error: {e}");
                let _ = tx.send(Err(e)).await;
            }
            info!("Gemini SSE stream ended");
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

    #[test]
    fn test_convert_messages_keeps_only_leading_system_as_system_instruction() {
        let messages = vec![
            Message::text(Role::System, "stable prompt"),
            Message::text(Role::User, "question"),
            Message::text(Role::System, "runtime tail"),
        ];

        let (system, contents) = convert_messages(&messages);

        let system = system.expect("system instruction");
        assert_eq!(system.parts.len(), 1);
        match &system.parts[0] {
            GeminiPartV2::Text { text } => assert_eq!(text, "stable prompt"),
            _ => panic!("expected text system part"),
        }
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0].role, "user");
        assert_eq!(contents[1].role, "user");
        match &contents[1].parts[0] {
            GeminiPartV2::Text { text } => assert_eq!(text, "runtime tail"),
            _ => panic!("expected text context part"),
        }
    }

    #[test]
    fn test_convert_messages_maps_tool_call_id_to_function_name() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                parts: vec![],
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "call_0".to_string(),
                    name: "search_knowledge_base".to_string(),
                    arguments: r#"{"query":"rust"}"#.to_string(),
                    thought_signature: None,
                }]),
                reasoning_content: None,
            },
            Message::text_with_name(Role::Tool, r#"{"ok":true}"#, "call_0"),
        ];

        let (_system, contents) = convert_messages(&messages);
        let last = contents.last().expect("expected tool response message");
        assert_eq!(last.role, "user");
        let part = last.parts.last().expect("expected function response part");
        match part {
            GeminiPartV2::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "search_knowledge_base");
            }
            _ => panic!("expected FunctionResponse part"),
        }
    }

    #[test]
    fn test_convert_messages_wraps_non_object_tool_result() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                parts: vec![],
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "call_0".to_string(),
                    name: "write_note".to_string(),
                    arguments: r#"{"filename":"a.md"}"#.to_string(),
                    thought_signature: None,
                }]),
                reasoning_content: None,
            },
            Message::text_with_name(Role::Tool, "plain text result", "call_0"),
        ];

        let (_system, contents) = convert_messages(&messages);
        let last = contents.last().expect("expected tool response message");
        let part = last.parts.last().expect("expected function response part");
        match part {
            GeminiPartV2::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "write_note");
                assert!(function_response.response.is_object());
                assert_eq!(
                    function_response.response["content"],
                    serde_json::Value::String("plain text result".to_string())
                );
            }
            _ => panic!("expected FunctionResponse part"),
        }
    }

    #[test]
    fn test_to_incremental_delta_handles_cumulative_and_delta_chunks() {
        let mut previous = String::new();

        // Cumulative chunk: first full snapshot.
        assert_eq!(
            to_incremental_delta(&mut previous, "Hello".to_string()),
            "Hello"
        );
        // Cumulative chunk: emit only appended suffix.
        assert_eq!(
            to_incremental_delta(&mut previous, "Hello world".to_string()),
            " world"
        );
        // Already-delta chunk: preserve as-is.
        assert_eq!(to_incremental_delta(&mut previous, "!".to_string()), "!");
    }

    #[test]
    fn test_build_request_body_enables_include_thoughts_when_thinking_enabled() {
        let request = CompletionRequest {
            model: "gemini-2.5-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.2),
            max_tokens: Some(256),
            tools: None,
            stop: None,
            thinking_budget: Some(2048),
            reasoning_effort: None,
            provider_type: None,
            parallel_tool_calls: true,
        };

        let body = build_request_body(&request, None, vec![]);
        let gc = body.generation_config.expect("generation config");
        let tc = gc.thinking_config.expect("thinking config");
        assert_eq!(tc.include_thoughts, Some(true));
        assert_eq!(tc.thinking_budget, Some(2048));
    }

    #[test]
    fn test_push_utf8_stream_chunk_handles_split_multibyte_chars() {
        let text = "data: {\"text\":\"你好，Gemini 🙂\"}\n\n";
        let bytes = text.as_bytes();

        for split in 1..bytes.len() {
            let mut buffer = String::new();
            let mut pending = Vec::new();

            push_utf8_stream_chunk(&mut buffer, &mut pending, &bytes[..split]).unwrap();
            push_utf8_stream_chunk(&mut buffer, &mut pending, &bytes[split..]).unwrap();

            assert_eq!(buffer, text);
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn test_push_utf8_stream_chunk_rejects_invalid_utf8() {
        let mut buffer = String::new();
        let mut pending = Vec::new();

        let err = push_utf8_stream_chunk(&mut buffer, &mut pending, &[0xff])
            .expect_err("invalid byte should be rejected");
        assert!(err.to_string().contains("Invalid UTF-8 in stream"));
    }

    #[test]
    fn test_split_delta_for_streaming_preserves_text_without_breaking_multibyte_chars() {
        let text = "这是一个比较长的 Gemini streaming delta，用来确认中文和 emoji 🙂 不会被切坏。";
        let parts = split_delta_for_streaming(text, 8);

        assert!(parts.len() > 1);
        assert_eq!(parts.join(""), text);
        assert!(parts
            .iter()
            .all(|part| std::str::from_utf8(part.as_bytes()).is_ok()));
    }

    #[test]
    fn test_split_delta_for_streaming_keeps_small_delta_single_chunk() {
        let parts = split_delta_for_streaming("hello", 24);
        assert_eq!(parts, vec!["hello".to_string()]);
    }
}
