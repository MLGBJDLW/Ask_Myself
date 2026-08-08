//! Google Gemini LLM provider.
//!
//! Uses the Gemini REST API with API key authentication via query parameter.
//! System prompts use top-level `systemInstruction`, roles map "assistant" → "model",
//! and tool calls use `functionCall`/`functionResponse` parts.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{error, info};

use super::reasoning_profile::{resolve_reasoning_profile, ReasoningApiStyle};
use super::transport::{shared_http_transport, HttpTransport};
use super::{
    configured_request_timeout, next_stream_item_with_idle_timeout, send_stream_start_request,
    serialized_json_body, with_request_timeout, CompletionRequest, CompletionResponse, ContentPart,
    FinishReason, LlmProvider, Message, ProviderConfig, ProviderType, ReasoningEffort, Role,
    StreamChunk, ToolCallDelta, ToolCallRequest, ToolDefinition, Usage,
    DEFAULT_STREAM_IDLE_TIMEOUT,
};
use crate::error::CoreError;
use crate::provider_catalog::model_limits_from_catalog;
use std::sync::Arc;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const SYNTHETIC_CALL_ID_PREFIX: &str = "__nexa_gemini_missing_call_id_";

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
        #[serde(
            rename = "thoughtSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        thought_signature: Option<String>,
    },
    Text {
        text: String,
        #[serde(
            rename = "thoughtSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        thought_signature: Option<String>,
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
    /// Preserve forward-compatible response parts so a newly introduced
    /// Google part does not make the entire candidate fail deserialization.
    Unknown(serde_json::Value),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct GeminiBlob {
    mime_type: String,
    data: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct GeminiFunctionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
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

fn push_or_merge_content(
    contents: &mut Vec<GeminiContentV2>,
    role: &str,
    parts: Vec<GeminiPartV2>,
) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut() {
        if last.role == role {
            last.parts.extend(parts);
            return;
        }
    }
    contents.push(GeminiContentV2 {
        role: role.to_string(),
        parts,
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequestV2 {
    contents: Vec<GeminiContentV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstructionV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
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
    thinking_level: Option<String>,
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
    #[serde(default)]
    prompt_feedback: Option<GeminiPromptFeedback>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
    finish_reason: Option<String>,
    #[serde(default)]
    finish_message: Option<String>,
    #[serde(default)]
    grounding_metadata: Option<GeminiGroundingMetadata>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGroundingMetadata {
    #[serde(default)]
    web_search_queries: Vec<String>,
    #[serde(default)]
    grounding_chunks: Vec<GeminiGroundingChunk>,
    #[serde(default)]
    grounding_supports: Vec<GeminiGroundingSupport>,
}

#[derive(Clone, Deserialize)]
struct GeminiGroundingChunk {
    #[serde(default)]
    web: Option<GeminiGroundingWeb>,
}

#[derive(Clone, Deserialize)]
struct GeminiGroundingWeb {
    uri: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGroundingSupport {
    #[serde(default)]
    segment: Option<GeminiGroundingSegment>,
    #[serde(default)]
    grounding_chunk_indices: Vec<usize>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGroundingSegment {
    #[serde(default)]
    start_index: Option<u32>,
    #[serde(default)]
    end_index: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPromptFeedback {
    #[serde(default)]
    block_reason: Option<String>,
    #[serde(default)]
    block_reason_message: Option<String>,
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
    cached_content_token_count: Option<u32>,
    #[serde(default)]
    thoughts_token_count: Option<i64>,
    #[serde(default)]
    tool_use_prompt_token_count: Option<u32>,
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
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiModel {
    name: String,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY"
        | "RECITATION"
        | "LANGUAGE"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn normalized_model_name(model: &str) -> &str {
    model.strip_prefix("models/").unwrap_or(model)
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
                    system_parts.push(GeminiPartV2::Text {
                        text,
                        thought_signature: None,
                    });
                } else if !text.is_empty() {
                    push_or_merge_content(
                        &mut contents,
                        "user",
                        vec![GeminiPartV2::Text {
                            text,
                            thought_signature: None,
                        }],
                    );
                }
            }
            Role::User => {
                let parts: Vec<GeminiPartV2> = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(GeminiPartV2::Text {
                            text: text.clone(),
                            thought_signature: None,
                        }),
                        ContentPart::Image { media_type, data } => Some(GeminiPartV2::InlineData {
                            inline_data: GeminiBlob {
                                mime_type: media_type.clone(),
                                data: data.clone(),
                            },
                        }),
                        ContentPart::ProviderTurn { .. } => None,
                    })
                    .collect();
                push_or_merge_content(&mut contents, "user", parts);
            }
            Role::Assistant => {
                let mut parts: Vec<GeminiPartV2> = Vec::new();
                let text = msg.text_content();
                if !text.is_empty() {
                    parts.push(GeminiPartV2::Text {
                        text,
                        thought_signature: None,
                    });
                }
                if let Some(ref calls) = msg.tool_calls {
                    for tc in calls {
                        tool_id_to_name.insert(tc.id.clone(), tc.name.clone());
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                        parts.push(GeminiPartV2::FunctionCall {
                            function_call: GeminiFunctionCall {
                                id: Some(tc.id.clone()),
                                name: tc.name.clone(),
                                args,
                            },
                            thought_signature: tc
                                .thought_signature
                                .as_deref()
                                .map(super::provider_turn::raw_gemini_thought_signature),
                        });
                    }
                }
                if !parts.is_empty() && contents.is_empty() {
                    // Context compaction can retain an assistant function-call
                    // turn while dropping the user turn that preceded it.
                    // Gemini rejects a leading model/function-call turn, so
                    // restore the structural boundary without inventing task
                    // content or altering signed function-call parts.
                    push_or_merge_content(
                        &mut contents,
                        "user",
                        vec![GeminiPartV2::Text {
                            text: "[Retained conversation context begins here.]".to_string(),
                            thought_signature: None,
                        }],
                    );
                }
                push_or_merge_content(&mut contents, "model", parts);
            }
            Role::Tool => {
                // Gemini expects function responses as user-role parts.
                let tool_ref = msg.name.clone().unwrap_or_default();
                let Some(tool_name) = tool_id_to_name.get(&tool_ref).cloned() else {
                    // Compaction can retain a tool result without its function-call
                    // predecessor. A call id is not a valid function name; retain the
                    // result as ordinary context instead of sending malformed protocol.
                    let text = msg.text_content();
                    push_or_merge_content(
                        &mut contents,
                        "user",
                        vec![GeminiPartV2::Text {
                            text: format!("[Retained tool result for {tool_ref}]\n{text}"),
                            thought_signature: None,
                        }],
                    );
                    continue;
                };

                // Gemini requires an object-like payload for functionResponse.response.
                let text = msg.text_content();
                let mut response_val: serde_json::Value = serde_json::from_str(&text)
                    .unwrap_or_else(|_| serde_json::json!({ "content": text }));
                if !response_val.is_object() {
                    response_val = serde_json::json!({ "content": response_val });
                }

                let make_part = || GeminiPartV2::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        id: Some(tool_ref.clone()),
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
                    push_or_merge_content(&mut contents, "user", vec![make_part()]);
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

fn convert_tools(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    let has_native_search = tools
        .iter()
        .any(crate::llm::native_search::is_native_marker);
    let send_local_search = crate::llm::native_search::should_send_local_search(tools);
    let declarations = tools
        .iter()
        .filter(|tool| !crate::llm::native_search::is_native_marker(tool))
        .filter(|tool| {
            send_local_search || tool.name != crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL
        })
        .map(|tool| GeminiFunctionDeclaration {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: clean_schema_for_gemini(&tool.parameters),
        })
        .collect::<Vec<_>>();
    let mut converted = Vec::new();
    if !declarations.is_empty() {
        converted.push(serde_json::json!({ "functionDeclarations": declarations }));
    }
    if has_native_search {
        converted.push(serde_json::json!({ "googleSearch": {} }));
    }
    converted
}

/// Returns `true` if the model supports extended thinking (`thinking_config`).
/// Gemini 2.5 uses token budgets; Gemini 3 and later use effort levels.
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

fn uses_thinking_levels(model: &str) -> bool {
    let name = model
        .strip_prefix("models/")
        .unwrap_or(model)
        .to_ascii_lowercase();

    name.strip_prefix("gemini-")
        .and_then(|rest| rest.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 3)
}

fn uses_latest_generation_contract(model: &str) -> bool {
    let name = model
        .strip_prefix("models/")
        .unwrap_or(model)
        .to_ascii_lowercase();

    if name.starts_with("gemini-3.5-flash-lite") {
        return true;
    }

    let Some(version) = name.strip_prefix("gemini-") else {
        return false;
    };
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts.next().and_then(|part| {
        part.split(|character: char| !character.is_ascii_digit())
            .next()
            .and_then(|digits| digits.parse::<u32>().ok())
    });

    matches!((major, minor), (Some(major), _) if major > 3)
        || matches!((major, minor), (Some(3), Some(minor)) if minor >= 6)
}

fn normalize_thinking_level(level: &ReasoningEffort) -> String {
    match level {
        ReasoningEffort::None | ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max => "high",
    }
    .to_string()
}

fn thinking_budget_to_level(budget: u32) -> String {
    match budget {
        ..=128 => "minimal",
        129..=4_096 => "low",
        4_097..=16_384 => "medium",
        _ => "high",
    }
    .to_string()
}

fn normalize_thinking_budget(model: &str, budget: u32) -> i32 {
    let name = normalized_model_name(model).to_ascii_lowercase();
    if name.starts_with("gemini-2.5-flash-lite") {
        return if budget == 0 {
            0
        } else {
            budget.clamp(512, 24_576) as i32
        };
    }
    if name.starts_with("gemini-2.5-flash") {
        return budget.min(24_576) as i32;
    }
    if name.starts_with("gemini-2.5-pro") {
        return budget.clamp(128, 32_768) as i32;
    }
    budget.min(i32::MAX as u32) as i32
}

/// Nexa policy: keep a bounded share of Gemini's output budget available for
/// answer-channel text. Google reports thought tokens separately, but they are
/// still generated within the response budget; this clamp is a client-side
/// safety margin rather than a Gemini protocol guarantee.
fn thinking_budget_with_answer_reserve(
    model: &str,
    budget: u32,
    max_output_tokens: Option<u32>,
) -> Option<i32> {
    let normalized = normalize_thinking_budget(model, budget).max(0) as u32;
    if normalized == 0 {
        return Some(0);
    }
    let Some(max_output_tokens) = max_output_tokens else {
        return Some(normalized.min(i32::MAX as u32) as i32);
    };

    let answer_reserve = (max_output_tokens / 4)
        .clamp(128, 2_048)
        .min(max_output_tokens);
    let thinking_ceiling = max_output_tokens.saturating_sub(answer_reserve);
    let model_name = normalized_model_name(model).to_ascii_lowercase();
    let minimum_supported_budget = if model_name.starts_with("gemini-2.5-flash-lite") {
        512
    } else if model_name.starts_with("gemini-2.5-pro") {
        128
    } else {
        0
    };

    if thinking_ceiling < minimum_supported_budget {
        return None;
    }

    Some(normalized.min(thinking_ceiling).min(i32::MAX as u32) as i32)
}

fn answer_budget_basis(request: &CompletionRequest) -> Option<u32> {
    request.max_tokens.or_else(|| {
        model_limits_from_catalog(ProviderType::Google, &request.model)
            .and_then(|limits| limits.max_output_tokens)
            .and_then(|limit| u32::try_from(limit).ok())
    })
}

fn build_request_body(
    request: &CompletionRequest,
    system_instruction: Option<GeminiSystemInstructionV2>,
    contents: Vec<GeminiContentV2>,
) -> GeminiRequestV2 {
    // Only send thinking_config to models that support it (Gemini 2.5+).
    let thinking_config = if supports_thinking(&request.model) {
        if uses_thinking_levels(&request.model) {
            request
                .reasoning_effort
                .as_ref()
                .map(normalize_thinking_level)
                .or_else(|| request.thinking_budget.map(thinking_budget_to_level))
                .map(|thinking_level| GeminiThinkingConfig {
                    thinking_budget: None,
                    thinking_level: Some(thinking_level),
                    // Required to receive `thought: true` parts in streaming/non-streaming responses.
                    include_thoughts: Some(true),
                })
        } else {
            request.thinking_budget.and_then(|budget| {
                thinking_budget_with_answer_reserve(
                    &request.model,
                    budget,
                    answer_budget_basis(request),
                )
                .map(|thinking_budget| GeminiThinkingConfig {
                    thinking_budget: Some(thinking_budget),
                    thinking_level: None,
                    // Required to receive `thought: true` parts in streaming/non-streaming responses.
                    include_thoughts: Some(true),
                })
            })
        }
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
            temperature: if has_thinking || uses_latest_generation_contract(&request.model) {
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

fn prompt_block_error(resp: &GeminiResponse) -> Option<CoreError> {
    let feedback = resp.prompt_feedback.as_ref()?;
    let reason = feedback.block_reason.as_deref()?;
    let detail = feedback
        .block_reason_message
        .as_deref()
        .filter(|message| !message.trim().is_empty())
        .map(|message| format!(": {message}"))
        .unwrap_or_default();
    Some(CoreError::Llm(format!(
        "Gemini blocked the prompt ({reason}){detail}"
    )))
}

type GeminiExtractedResponse = (
    String,
    Vec<ToolCallRequest>,
    FinishReason,
    Usage,
    Option<String>,
);

fn extract_search_evidence(resp: &GeminiResponse) -> Option<super::native_search::SearchEvidence> {
    let metadata = resp
        .candidates
        .as_ref()?
        .first()?
        .grounding_metadata
        .as_ref()?;
    let mut citations = Vec::new();
    let mut cited_chunks = HashSet::new();
    for support in &metadata.grounding_supports {
        for index in &support.grounding_chunk_indices {
            let Some(web) = metadata
                .grounding_chunks
                .get(*index)
                .and_then(|chunk| chunk.web.as_ref())
            else {
                continue;
            };
            cited_chunks.insert(*index);
            citations.push(super::native_search::SearchCitation {
                url: web.uri.clone(),
                title: web.title.clone(),
                start_index: support
                    .segment
                    .as_ref()
                    .and_then(|segment| segment.start_index),
                end_index: support
                    .segment
                    .as_ref()
                    .and_then(|segment| segment.end_index),
            });
        }
    }
    for (index, chunk) in metadata.grounding_chunks.iter().enumerate() {
        let Some(web) = chunk.web.as_ref() else {
            continue;
        };
        if cited_chunks.insert(index) {
            citations.push(super::native_search::SearchCitation {
                url: web.uri.clone(),
                title: web.title.clone(),
                start_index: None,
                end_index: None,
            });
        }
    }
    (!citations.is_empty()).then(|| super::native_search::SearchEvidence {
        dialect: crate::model_catalog::NativeSearchDialect::GeminiGoogleSearch,
        query: (!metadata.web_search_queries.is_empty())
            .then(|| metadata.web_search_queries.join(" | ")),
        citations,
    })
}

/// Extract text, tool calls, finish reason, and usage from a Gemini response.
fn extract_response(resp: &GeminiResponse) -> Result<GeminiExtractedResponse, CoreError> {
    if resp
        .candidates
        .as_ref()
        .is_none_or(|candidates| candidates.is_empty())
    {
        if let Some(error) = prompt_block_error(resp) {
            return Err(error);
        }
    }
    let candidate = resp.candidates.as_ref().and_then(|c| c.first());

    let mut text_parts = Vec::new();
    let mut thinking_parts = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(candidate) = candidate {
        if let Some(ref content) = candidate.content {
            if let Some(ref parts) = content.parts {
                for (idx, part) in parts.iter().enumerate() {
                    match part {
                        GeminiPartV2::Thought {
                            text,
                            thought,
                            thought_signature,
                        } if *thought => {
                            thinking_parts.push(text.clone());
                            let _ = thought_signature;
                        }
                        GeminiPartV2::Thought {
                            text,
                            thought_signature,
                            ..
                        }
                        | GeminiPartV2::Text {
                            text,
                            thought_signature,
                        } => {
                            text_parts.push(text.clone());
                            let _ = thought_signature;
                        }
                        GeminiPartV2::FunctionCall {
                            function_call,
                            thought_signature,
                        } => {
                            let tool_call_id = function_call
                                .id
                                .clone()
                                .unwrap_or_else(|| format!("{SYNTHETIC_CALL_ID_PREFIX}{idx}"));
                            let captured_signature =
                                thought_signature.as_ref().and_then(|signature| {
                                    super::provider_turn::encode_gemini_thought_signature(
                                        &super::provider_turn::GeminiThoughtSignature {
                                            tool_call_id: tool_call_id.clone(),
                                            model_part_index: Some(idx),
                                            signature: signature.clone(),
                                        },
                                    )
                                });
                            tool_calls.push(ToolCallRequest {
                                id: tool_call_id,
                                name: function_call.name.clone(),
                                arguments: serde_json::to_string(&function_call.args)
                                    .unwrap_or_default(),
                                thought_signature: captured_signature,
                            });
                        }
                        GeminiPartV2::FunctionResponse { .. } | GeminiPartV2::InlineData { .. } => {
                        }
                        GeminiPartV2::Unknown(value) => {
                            tracing::debug!(part = %value, "Ignoring unsupported Gemini response part");
                        }
                    }
                }
            }
        }
    }

    if let Some(candidate) = candidate {
        if let (Some(reason), Some(message)) = (
            candidate.finish_reason.as_deref(),
            candidate
                .finish_message
                .as_deref()
                .filter(|message| !message.trim().is_empty()),
        ) {
            if !matches!(reason, "STOP" | "MAX_TOKENS") {
                return Err(CoreError::Llm(format!(
                    "Gemini stopped generation ({reason}): {message}"
                )));
            }
        }
        if text_parts.is_empty() && tool_calls.is_empty() {
            if let Some(reason) = candidate.finish_reason.as_deref() {
                if !matches!(reason, "STOP" | "MAX_TOKENS") {
                    let detail = candidate
                        .finish_message
                        .as_deref()
                        .filter(|message| !message.trim().is_empty())
                        .map(|message| format!(": {message}"))
                        .unwrap_or_default();
                    return Err(CoreError::Llm(format!(
                        "Gemini stopped generation ({reason}){detail}"
                    )));
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
        .map(|u| {
            let tool_prompt_tokens = u.tool_use_prompt_token_count.unwrap_or(0);
            let prompt_tokens = u
                .prompt_token_count
                .unwrap_or(0)
                .saturating_add(tool_prompt_tokens);
            let completion_tokens = u.candidates_token_count.unwrap_or(0);
            let thinking_tokens = u.thoughts_token_count.map(|tokens| tokens.max(0) as u32);
            let accounted_total = prompt_tokens
                .saturating_add(completion_tokens)
                .saturating_add(thinking_tokens.unwrap_or(0));
            Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: u.total_token_count.unwrap_or(0).max(accounted_total),
                thinking_tokens,
                tool_prompt_tokens: u.tool_use_prompt_token_count,
                cache_read_tokens: u.cached_content_token_count,
                cache_miss_tokens: None,
                cache_creation_tokens: None,
                provider_raw: None,
            }
        })
        .unwrap_or_default();

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join(""))
    };

    Ok((
        text_parts.join(""),
        tool_calls,
        finish_reason,
        usage,
        thinking,
    ))
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

async fn send_gemini_content_chunks(
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    text_delta: String,
    thinking_delta: Option<String>,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
) -> bool {
    let chunk = StreamChunk {
        delta: text_delta,
        tool_call_delta: None,
        finish_reason,
        usage,
        thinking_delta,
    };
    let has_payload = !chunk.delta.is_empty()
        || chunk
            .thinking_delta
            .as_deref()
            .is_some_and(|delta| !delta.is_empty())
        || chunk.finish_reason.is_some()
        || chunk.usage.is_some();
    if has_payload && tx.send(Ok(chunk)).await.is_err() {
        return false;
    }

    true
}

#[derive(Default)]
struct GeminiToolCallStreamState {
    pending: Vec<ToolCallRequest>,
    provider_indices: HashMap<String, usize>,
    synthetic_indices: HashMap<String, usize>,
    next_synthetic_id: u64,
}

impl GeminiToolCallStreamState {
    fn record_snapshot(&mut self, mut tool_call: ToolCallRequest) -> Result<(), CoreError> {
        if tool_call.id.starts_with(SYNTHETIC_CALL_ID_PREFIX) {
            if let Some(index) = self.synthetic_indices.get(&tool_call.id).copied() {
                if self.pending[index].name == tool_call.name {
                    if self.pending[index].arguments != tool_call.arguments {
                        return Err(CoreError::StreamIncomplete(
                            "Gemini streamed ambiguous id-less function-call snapshots; retrying without streaming"
                                .to_string(),
                        ));
                    }
                    if tool_call.thought_signature.is_none() {
                        tool_call.thought_signature = self.pending[index].thought_signature.clone();
                    }
                    tool_call.id = self.pending[index].id.clone();
                    self.pending[index] = tool_call;
                    return Ok(());
                }
            }

            let provider_position = tool_call.id.clone();
            tool_call.id = format!("call_{}", self.next_synthetic_id);
            self.next_synthetic_id += 1;
            self.synthetic_indices
                .insert(provider_position, self.pending.len());
            self.pending.push(tool_call);
            return Ok(());
        }

        if let Some(index) = self.provider_indices.get(&tool_call.id).copied() {
            if tool_call.thought_signature.is_none() {
                tool_call.thought_signature = self.pending[index].thought_signature.clone();
            }
            self.pending[index] = tool_call;
        } else {
            self.provider_indices
                .insert(tool_call.id.clone(), self.pending.len());
            self.pending.push(tool_call);
        }
        Ok(())
    }

    fn take_pending(&mut self) -> Vec<ToolCallRequest> {
        self.provider_indices.clear();
        self.synthetic_indices.clear();
        std::mem::take(&mut self.pending)
    }
}

async fn send_gemini_tool_call(
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    tool_call: ToolCallRequest,
) -> bool {
    let chunk = StreamChunk {
        delta: String::new(),
        tool_call_delta: Some(ToolCallDelta {
            id: tool_call.id.clone(),
            name: Some(tool_call.name),
            arguments_delta: tool_call.arguments,
            index: tool_call
                .id
                .strip_prefix("call_")
                .and_then(|value| value.parse::<u32>().ok()),
            thought_signature: tool_call.thought_signature,
        }),
        finish_reason: None,
        usage: None,
        thinking_delta: None,
    };
    tx.send(Ok(chunk)).await.is_ok()
}

async fn emit_gemini_response_chunk(
    resp: GeminiResponse,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    tool_call_state: &mut GeminiToolCallStreamState,
    saw_finish_reason: &mut bool,
) -> Result<bool, CoreError> {
    let citation_appendix = extract_search_evidence(&resp)
        .as_ref()
        .map(super::native_search::render_citation_appendix)
        .unwrap_or_default();
    let has_finish = resp
        .candidates
        .as_ref()
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.finish_reason.as_ref())
        .is_some();
    let (mut text_delta, tool_calls, finish_reason, usage, thinking_delta) =
        extract_response(&resp)?;
    text_delta.push_str(&citation_appendix);
    if has_finish {
        *saw_finish_reason = true;
    }

    for tool_call in tool_calls {
        tool_call_state.record_snapshot(tool_call)?;
    }

    if has_finish && tool_call_state.pending.is_empty() {
        let usage = (usage.total_tokens > 0).then_some(usage);
        return Ok(send_gemini_content_chunks(
            tx,
            text_delta,
            thinking_delta,
            Some(finish_reason),
            usage,
        )
        .await);
    }

    if (!text_delta.is_empty() || thinking_delta.is_some())
        && !send_gemini_content_chunks(tx, text_delta, thinking_delta, None, None).await
    {
        return Ok(false);
    }

    if !has_finish {
        if usage.total_tokens > 0
            && !send_gemini_content_chunks(tx, String::new(), None, None, Some(usage)).await
        {
            return Ok(false);
        }
        return Ok(true);
    }

    for tool_call in tool_call_state.take_pending() {
        if !send_gemini_tool_call(tx, tool_call).await {
            return Ok(false);
        }
    }

    let usage = (usage.total_tokens > 0).then_some(usage);
    Ok(send_gemini_content_chunks(tx, String::new(), None, Some(finish_reason), usage).await)
}

async fn process_gemini_sse_event(
    data: &str,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    tool_call_state: &mut GeminiToolCallStreamState,
    saw_finish_reason: &mut bool,
) -> Result<bool, CoreError> {
    let data = data.trim();
    if data.is_empty() {
        return Ok(true);
    }
    if data == "[DONE]" {
        return Ok(false);
    }

    let resp: GeminiResponse = serde_json::from_str(data)
        .map_err(|e| CoreError::Llm(format!("Malformed Gemini SSE event: {e}")))?;

    emit_gemini_response_chunk(resp, tx, tool_call_state, saw_finish_reason).await
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
    let mut tool_call_state = GeminiToolCallStreamState::default();
    let mut saw_finish_reason = false;

    'stream: while let Some(chunk_result) = next_stream_item_with_idle_timeout(
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
                    &mut tool_call_state,
                    &mut saw_finish_reason,
                )
                .await?
                {
                    break 'stream;
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
        let _ = process_gemini_sse_event(&data, &tx, &mut tool_call_state, &mut saw_finish_reason)
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
    transport: Arc<HttpTransport>,
    config: ProviderConfig,
    request_timeout: Option<Duration>,
}

impl GeminiProvider {
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

    fn reasoning_replay_policy(
        &self,
        model: &str,
    ) -> super::reasoning_profile::ReasoningReplayPolicy {
        resolve_reasoning_profile(
            self.config.provider_type,
            self.config.base_url.as_deref(),
            ReasoningApiStyle::GeminiGenerateContent,
            model,
        )
        .replay_policy
    }

    fn route_snapshot(&self, request: &CompletionRequest) -> super::provider_turn::RouteSnapshot {
        let profile = resolve_reasoning_profile(
            self.config.provider_type,
            self.config.base_url.as_deref(),
            ReasoningApiStyle::GeminiGenerateContent,
            &request.model,
        );
        let mut snapshot =
            super::provider_turn::RouteSnapshot::from_profile_for_request(&profile, request);
        if uses_thinking_levels(&request.model) {
            // Gemini 3 function-calling turns require a thought signature even
            // when the client does not request visible thinking.
            snapshot.replay_policy =
                super::reasoning_profile::ReasoningReplayPolicy::OpaqueSignature;
        } else {
            // Gemini 2.5 signatures are optional, but any returned value is
            // still captured and replayed exactly.
            snapshot.replay_policy = super::reasoning_profile::ReasoningReplayPolicy::NotRequired;
        }
        snapshot
    }

    async fn list_models(&self) -> Result<Vec<String>, CoreError> {
        let api_key = self.api_key()?;
        let url = format!("{}/models", self.base_url());
        let mut page_token: Option<String> = None;
        let mut models = Vec::new();
        let mut seen_page_tokens = HashSet::new();

        loop {
            let mut request = self.transport.client().get(&url).query(&[("key", api_key)]);
            if let Some(token) = page_token.as_deref() {
                request = request.query(&[("pageToken", token)]);
            }
            let response = with_request_timeout(request, self.request_timeout)
                .send()
                .await
                .map_err(|e| CoreError::Llm(format!("Request failed: {e}")))?;
            let response = self.check_response(response).await?;
            let resp: GeminiListModelsResponse = response
                .json()
                .await
                .map_err(|e| CoreError::Llm(format!("Failed to parse models response: {e}")))?;

            models.extend(
                resp.models
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|model| {
                        model
                            .supported_generation_methods
                            .iter()
                            .any(|method| method == "generateContent")
                    })
                    .map(|model| normalized_model_name(&model.name).to_string()),
            );

            page_token = resp.next_page_token.filter(|token| !token.is_empty());
            if let Some(token) = page_token.as_ref() {
                if !seen_page_tokens.insert(token.clone()) {
                    return Err(CoreError::Llm(
                        "Gemini model listing repeated a pagination token".to_string(),
                    ));
                }
            }
            if page_token.is_none() {
                break;
            }
        }

        models.sort();
        models.dedup();
        Ok(models)
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse, CoreError> {
        let api_key = self.api_key()?;
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url(),
            normalized_model_name(&request.model),
            api_key,
        );

        let (system_instruction, contents) = convert_messages(&request.messages);
        let body = build_request_body(request, system_instruction, contents);
        let body_bytes = serialized_json_body(&body, "Gemini completion request")?;

        let response = with_request_timeout(
            self.transport
                .client()
                .post(&url)
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

        let resp: GeminiResponse = response
            .json()
            .await
            .inspect_err(|error| {
                self.transport.record_transport_failure(&error.to_string());
            })
            .map_err(|e| CoreError::Llm(format!("Failed to parse response: {e}")))?;
        self.transport.record_transport_success();

        let (mut content, tool_calls, finish_reason, usage, thinking) = extract_response(&resp)?;
        if let Some(evidence) = extract_search_evidence(&resp) {
            content.push_str(&super::native_search::render_citation_appendix(&evidence));
        }

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
            normalized_model_name(&request.model),
            api_key,
        );

        let (system_instruction, contents) = convert_messages(&request.messages);
        let body = build_request_body(request, system_instruction, contents);
        let body_bytes = serialized_json_body(&body, "Gemini stream request")?;

        info!(
            "Gemini stream request to {}..., model={}",
            &url[..url.find("key=").unwrap_or(url.len())],
            request.model
        );

        let response = send_stream_start_request(
            self.transport
                .client()
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body_bytes),
            self.request_timeout,
            "Gemini stream request",
        )
        .await
        .inspect_err(|e| {
            self.transport.record_transport_failure(&e.to_string());
            error!("Gemini stream send failed: {e}");
        })?;

        info!("Gemini stream response status: {}", response.status());
        let response = self.check_response(response).await?;

        let (tx, rx) = mpsc::channel(64);
        info!("Gemini SSE stream started");

        let transport = Arc::clone(&self.transport);
        tokio::spawn(async move {
            if let Err(e) = parse_gemini_stream(response, tx.clone()).await {
                transport.record_transport_failure(&e.to_string());
                error!("Gemini SSE stream error: {e}");
                let _ = tx.send(Err(e)).await;
            } else {
                transport.record_transport_success();
            }
            info!("Gemini SSE stream ended");
        });

        let stream = futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<(), CoreError> {
        if self.list_models().await?.is_empty() {
            return Err(CoreError::Llm(
                "Gemini returned no models that support generateContent".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_native_search_replaces_only_the_local_search_tool() {
        let local = ToolDefinition {
            name: crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL.to_string(),
            description: "Local search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let shell = ToolDefinition {
            name: "run_shell".to_string(),
            description: "Run shell".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let marker = crate::llm::native_search::NativeSearchPlan::resolve(
            crate::llm::native_search::SearchExecutionMode::ProviderNative,
            ProviderType::Google,
            None,
            "gemini-3.6-flash",
        )
        .marker()
        .unwrap();

        let tools = convert_tools(&[local, shell, marker]);

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["functionDeclarations"][0]["name"], "run_shell");
        assert_eq!(tools[1], serde_json::json!({"googleSearch": {}}));
    }

    #[test]
    fn hybrid_search_keeps_local_and_google_search_paths() {
        let local = ToolDefinition {
            name: crate::llm::native_search::LOCAL_WEB_SEARCH_TOOL.to_string(),
            description: "Local search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let marker = crate::llm::native_search::NativeSearchPlan::resolve(
            crate::llm::native_search::SearchExecutionMode::Hybrid,
            ProviderType::Google,
            None,
            "gemini-3.6-flash",
        )
        .marker()
        .unwrap();

        let tools = convert_tools(&[local, marker]);

        assert_eq!(tools[0]["functionDeclarations"][0]["name"], "web_search");
        assert_eq!(tools[1], serde_json::json!({"googleSearch": {}}));
    }

    #[test]
    fn grounding_metadata_normalizes_into_clickable_citations() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"text": "Grounded answer"}]},
                "finishReason": "STOP",
                "groundingMetadata": {
                    "webSearchQueries": ["nexa release"],
                    "groundingChunks": [
                        {"web": {"uri": "https://example.com/release", "title": "Release notes"}}
                    ],
                    "groundingSupports": [{
                        "segment": {"startIndex": 0, "endIndex": 15},
                        "groundingChunkIndices": [0]
                    }]
                }
            }]
        }))
        .unwrap();

        let evidence = extract_search_evidence(&response).expect("grounding evidence");
        assert_eq!(evidence.query.as_deref(), Some("nexa release"));
        assert_eq!(evidence.citations.len(), 1);
        assert_eq!(evidence.citations[0].start_index, Some(0));
        assert_eq!(evidence.citations[0].end_index, Some(15));
        assert!(
            crate::llm::native_search::render_citation_appendix(&evidence)
                .contains("[Release notes](https://example.com/release)")
        );
    }

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
            GeminiPartV2::Text { text, .. } => assert_eq!(text, "stable prompt"),
            _ => panic!("expected text system part"),
        }
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, "user");
        match &contents[0].parts[1] {
            GeminiPartV2::Text { text, .. } => assert_eq!(text, "runtime tail"),
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
                prompt_cache_hint: None,
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
                prompt_cache_hint: None,
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
    fn test_convert_messages_does_not_use_orphan_call_id_as_function_name() {
        let messages = vec![Message::text_with_name(
            Role::Tool,
            r#"{"content":"retained"}"#,
            "call_without_predecessor",
        )];

        let (_system, contents) = convert_messages(&messages);
        let encoded = serde_json::to_value(contents).expect("serialize contents");

        assert!(encoded[0]["parts"][0].get("functionResponse").is_none());
        assert!(encoded[0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("call_without_predecessor"));
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
            reasoning_enabled: Some(true),
            reasoning_effort: None,
            provider_type: None,
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = build_request_body(&request, None, vec![]);
        let gc = body.generation_config.expect("generation config");
        let tc = gc.thinking_config.expect("thinking config");
        assert_eq!(tc.include_thoughts, Some(true));
        assert_eq!(tc.thinking_budget, Some(128));
        assert_eq!(tc.thinking_level, None);
    }

    #[test]
    fn test_thinking_budget_reserves_output_capacity_for_final_answer() {
        let request = CompletionRequest {
            model: "gemini-2.5-flash".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.2),
            max_tokens: Some(4096),
            tools: None,
            stop: None,
            thinking_budget: Some(4096),
            reasoning_enabled: Some(true),
            reasoning_effort: None,
            provider_type: None,
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = build_request_body(&request, None, vec![]);
        let thinking = body
            .generation_config
            .and_then(|config| config.thinking_config)
            .expect("thinking config");

        assert_eq!(thinking.thinking_budget, Some(3072));
    }

    #[test]
    fn test_automatic_output_limit_still_reserves_gemini_answer_capacity() {
        let request = CompletionRequest {
            model: "gemini-2.5-pro".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.2),
            max_tokens: None,
            tools: None,
            stop: None,
            thinking_budget: Some(65_536),
            reasoning_enabled: Some(true),
            reasoning_effort: None,
            provider_type: Some(ProviderType::Google),
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = build_request_body(&request, None, vec![]);
        let generation = body.generation_config.expect("generation config");
        assert_eq!(generation.max_output_tokens, None);
        assert_eq!(
            generation
                .thinking_config
                .expect("thinking config")
                .thinking_budget,
            Some(32_768),
        );
    }

    #[test]
    fn test_thinking_budget_uses_model_specific_ranges() {
        assert_eq!(normalize_thinking_budget("gemini-2.5-pro", 0), 128);
        assert_eq!(
            normalize_thinking_budget("models/gemini-2.5-flash", 99_999),
            24_576
        );
        assert_eq!(normalize_thinking_budget("gemini-2.5-flash", 0), 0);
        assert_eq!(normalize_thinking_budget("gemini-2.5-flash-lite", 1), 512);
        assert_eq!(
            thinking_budget_with_answer_reserve("gemini-2.5-flash-lite", 0, Some(256)),
            Some(0),
        );
    }

    #[test]
    fn test_finish_reason_mapping_covers_google_content_blocks() {
        for reason in [
            "SAFETY",
            "RECITATION",
            "LANGUAGE",
            "BLOCKLIST",
            "PROHIBITED_CONTENT",
            "SPII",
            "IMAGE_SAFETY",
            "IMAGE_PROHIBITED_CONTENT",
            "IMAGE_RECITATION",
        ] {
            assert_eq!(parse_finish_reason(reason), FinishReason::ContentFilter);
        }
    }

    #[test]
    fn test_thought_only_max_tokens_keeps_answer_channel_empty() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [
                    {"text": "raw internal reasoning", "thought": true}
                ]},
                "finishReason": "MAX_TOKENS"
            }]
        }))
        .expect("response");

        let (answer, tool_calls, finish_reason, _, thinking) =
            extract_response(&response).expect("extract response");

        assert!(answer.is_empty());
        assert!(tool_calls.is_empty());
        assert_eq!(finish_reason, FinishReason::Length);
        assert_eq!(thinking.as_deref(), Some("raw internal reasoning"));
    }

    #[test]
    fn test_unknown_response_part_does_not_discard_supported_parts() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [
                    {"fileData": {"mimeType": "application/pdf", "fileUri": "gs://example"}},
                    {"text": "usable text"}
                ]},
                "finishReason": "STOP"
            }]
        }))
        .expect("response");

        let (text, _, _, _, _) = extract_response(&response).expect("extract response");
        assert_eq!(text, "usable text");
    }

    #[test]
    fn test_latest_models_use_thinking_level_and_omit_temperature() {
        let request = CompletionRequest {
            model: "gemini-3.6-flash".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.2),
            max_tokens: Some(256),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::High),
            provider_type: None,
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = build_request_body(&request, None, vec![]);
        let config = body.generation_config.expect("generation config");
        let thinking = config.thinking_config.expect("thinking config");

        assert_eq!(config.temperature, None);
        assert_eq!(thinking.thinking_budget, None);
        assert_eq!(thinking.thinking_level.as_deref(), Some("high"));
        assert_eq!(thinking.include_thoughts, Some(true));
    }

    #[test]
    fn test_latest_models_omit_temperature_without_explicit_thinking() {
        let request = CompletionRequest {
            model: "gemini-3.5-flash-lite".to_string(),
            messages: vec![Message::text(Role::User, "hello")],
            temperature: Some(0.2),
            max_tokens: Some(256),
            tools: None,
            stop: None,
            thinking_budget: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_type: None,
            routing_session_id: None,
            parallel_tool_calls: true,
        };

        let body = build_request_body(&request, None, vec![]);
        let config = body.generation_config.expect("generation config");

        assert_eq!(config.temperature, None);
        assert!(config.thinking_config.is_none());
    }

    #[test]
    fn test_convert_messages_preserves_function_call_ids() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                parts: vec![],
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "fc_123".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"README.md"}"#.to_string(),
                    thought_signature: Some("signature".to_string()),
                }]),
                reasoning_content: None,
                prompt_cache_hint: None,
            },
            Message::text_with_name(Role::Tool, r#"{"content":"ok"}"#, "fc_123"),
        ];

        let (_system, contents) = convert_messages(&messages);
        let encoded = serde_json::to_value(contents).expect("Gemini contents should serialize");

        assert_eq!(encoded[0]["role"], "user");
        assert_eq!(encoded[1]["parts"][0]["functionCall"]["id"], "fc_123");
        assert_eq!(encoded[1]["parts"][0]["thoughtSignature"], "signature");
        assert_eq!(encoded[2]["parts"][0]["functionResponse"]["id"], "fc_123");
        assert_eq!(
            encoded[2]["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
    }

    #[test]
    fn test_convert_messages_merges_consecutive_model_turns_before_function_response() {
        let messages = vec![
            Message::text(Role::User, "Please investigate"),
            Message::text(Role::Assistant, "I will inspect this."),
            Message {
                role: Role::Assistant,
                parts: vec![],
                name: None,
                tool_calls: Some(vec![ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"src/main.rs"}"#.to_string(),
                    thought_signature: Some("signed-call".to_string()),
                }]),
                reasoning_content: None,
                prompt_cache_hint: None,
            },
            Message::text_with_name(Role::Tool, r#"{"content":"ok"}"#, "call_1"),
        ];

        let (_system, contents) = convert_messages(&messages);

        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0].role, "user");
        assert_eq!(contents[1].role, "model");
        assert_eq!(contents[1].parts.len(), 2);
        assert!(matches!(contents[1].parts[0], GeminiPartV2::Text { .. }));
        match &contents[1].parts[1] {
            GeminiPartV2::FunctionCall {
                function_call,
                thought_signature,
            } => {
                assert_eq!(function_call.id.as_deref(), Some("call_1"));
                assert_eq!(thought_signature.as_deref(), Some("signed-call"));
            }
            _ => panic!("expected function call"),
        }
        assert_eq!(contents[2].role, "user");
        assert!(matches!(
            contents[2].parts[0],
            GeminiPartV2::FunctionResponse { .. }
        ));
    }

    #[test]
    fn test_convert_messages_keeps_parallel_calls_and_responses_paired() {
        let messages = vec![
            Message::text(Role::User, "Inspect both files"),
            Message {
                role: Role::Assistant,
                parts: vec![],
                name: None,
                tool_calls: Some(vec![
                    ToolCallRequest {
                        id: "call_a".to_string(),
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"a.rs"}"#.to_string(),
                        thought_signature: None,
                    },
                    ToolCallRequest {
                        id: "call_b".to_string(),
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"b.rs"}"#.to_string(),
                        thought_signature: None,
                    },
                ]),
                reasoning_content: None,
                prompt_cache_hint: None,
            },
            Message::text_with_name(Role::Tool, r#"{"content":"a"}"#, "call_a"),
            Message::text_with_name(Role::Tool, r#"{"content":"b"}"#, "call_b"),
        ];

        let (_system, contents) = convert_messages(&messages);
        assert_eq!(
            contents
                .iter()
                .map(|content| content.role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "model", "user"]
        );
        assert_eq!(contents[1].parts.len(), 2);
        assert_eq!(contents[2].parts.len(), 2);
        let encoded = serde_json::to_value(contents).expect("serialize Gemini contents");
        assert_eq!(encoded[1]["parts"][0]["functionCall"]["id"], "call_a");
        assert_eq!(encoded[1]["parts"][1]["functionCall"]["id"], "call_b");
        assert_eq!(encoded[2]["parts"][0]["functionResponse"]["id"], "call_a");
        assert_eq!(encoded[2]["parts"][1]["functionResponse"]["id"], "call_b");
    }

    #[test]
    fn test_extract_response_preserves_provider_function_call_id() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "id": "fc_provider_1",
                            "name": "read_file",
                            "args": {"path": "README.md"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        }))
        .expect("response");

        let (_content, tool_calls, _finish_reason, _usage, _thinking) =
            extract_response(&response).expect("extract response");

        assert_eq!(tool_calls[0].id, "fc_provider_1");
    }

    #[test]
    fn test_extract_response_retains_signed_part_position_for_replay() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [
                    {"text": "I will inspect."},
                    {
                        "functionCall": {
                            "id": "fc_provider_1",
                            "name": "read_file",
                            "args": {"path": "README.md"}
                        },
                        "thoughtSignature": "opaque-signature"
                    }
                ]},
                "finishReason": "STOP"
            }]
        }))
        .expect("response");

        let (_, tool_calls, _, _, _) = extract_response(&response).expect("extract response");
        let captured = super::super::provider_turn::decode_gemini_thought_signature(
            tool_calls[0]
                .thought_signature
                .as_deref()
                .expect("captured signature"),
        )
        .expect("typed signature metadata");

        assert_eq!(captured.tool_call_id, "fc_provider_1");
        assert_eq!(captured.model_part_index, Some(1));
        assert_eq!(captured.signature, "opaque-signature");
    }

    #[test]
    fn test_extract_response_does_not_move_text_signature_to_function_call() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [
                    {"text": "I will inspect.", "thoughtSignature": "signed-context"},
                    {"functionCall": {"id": "fc_1", "name": "read_file", "args": {}}}
                ]},
                "finishReason": "STOP"
            }]
        }))
        .expect("response");

        let (_, tool_calls, _, _, _) = extract_response(&response).expect("extract response");

        assert_eq!(tool_calls[0].thought_signature, None);
    }

    #[test]
    fn test_extract_response_surfaces_prompt_block_reason() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "promptFeedback": {
                "blockReason": "PROHIBITED_CONTENT",
                "blockReasonMessage": "prompt policy rejected"
            }
        }))
        .expect("response");

        let error = extract_response(&response).expect_err("blocked prompt must be an error");

        assert!(error.to_string().contains("PROHIBITED_CONTENT"));
        assert!(error.to_string().contains("prompt policy rejected"));
    }

    #[test]
    fn test_extract_response_surfaces_candidate_block_message() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "finishReason": "SAFETY",
                "finishMessage": "response policy rejected"
            }]
        }))
        .expect("response");

        let error = extract_response(&response).expect_err("blocked response must be an error");

        assert!(error.to_string().contains("SAFETY"));
        assert!(error.to_string().contains("response policy rejected"));
    }

    #[test]
    fn test_extract_response_surfaces_finish_message_after_partial_text() {
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"text": "partial"}]},
                "finishReason": "MALFORMED_FUNCTION_CALL",
                "finishMessage": "arguments were invalid"
            }]
        }))
        .expect("response");

        let error = extract_response(&response).expect_err("protocol failure must be an error");

        assert!(error.to_string().contains("MALFORMED_FUNCTION_CALL"));
        assert!(error.to_string().contains("arguments were invalid"));
    }

    #[tokio::test]
    async fn test_malformed_sse_event_is_not_silently_skipped() {
        let (tx, _rx) = mpsc::channel(1);
        let mut tool_call_state = GeminiToolCallStreamState::default();
        let mut saw_finish_reason = false;

        let error = process_gemini_sse_event(
            "{not valid json}",
            &tx,
            &mut tool_call_state,
            &mut saw_finish_reason,
        )
        .await
        .expect_err("malformed SSE must fail the stream");

        assert!(error.to_string().contains("Malformed Gemini SSE event"));
    }

    #[tokio::test]
    async fn test_unknown_finish_reason_is_still_terminal() {
        let (tx, mut rx) = mpsc::channel(2);
        let mut tool_call_state = GeminiToolCallStreamState::default();
        let mut saw_finish_reason = false;
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"text": "partial result"}]},
                "finishReason": "FUTURE_REASON"
            }]
        }))
        .expect("response");

        assert!(emit_gemini_response_chunk(
            response,
            &tx,
            &mut tool_call_state,
            &mut saw_finish_reason,
        )
        .await
        .expect("emit response"));

        assert!(saw_finish_reason);
        assert_eq!(
            rx.recv().await.unwrap().unwrap().finish_reason,
            Some(FinishReason::Other)
        );
    }

    #[tokio::test]
    async fn test_repeated_tool_call_snapshots_emit_the_final_arguments() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut tool_call_state = GeminiToolCallStreamState::default();
        let mut saw_finish_reason = false;
        for (index, args) in [
            serde_json::json!({"path": "a"}),
            serde_json::json!({"path": "ab"}),
        ]
        .into_iter()
        .enumerate()
        {
            let response: GeminiResponse = serde_json::from_value(serde_json::json!({
                "candidates": [{
                    "content": {"parts": [{
                        "functionCall": {"id": "fc_1", "name": "read_file", "args": args}
                    }]},
                    "finishReason": (index == 1).then_some("STOP")
                }]
            }))
            .expect("response");
            assert!(emit_gemini_response_chunk(
                response,
                &tx,
                &mut tool_call_state,
                &mut saw_finish_reason,
            )
            .await
            .expect("emit response"));
        }

        let first = rx.recv().await.unwrap().unwrap();
        assert_eq!(
            first.tool_call_delta.unwrap().arguments_delta,
            r#"{"path":"ab"}"#
        );
        assert_eq!(
            rx.recv().await.unwrap().unwrap().finish_reason,
            Some(FinishReason::Stop)
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_missing_tool_call_ids_are_unique_across_sse_events() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut tool_call_state = GeminiToolCallStreamState::default();
        let mut saw_finish_reason = false;
        for (index, name) in ["read_file", "list_files"].into_iter().enumerate() {
            let response: GeminiResponse = serde_json::from_value(serde_json::json!({
                "candidates": [{
                    "content": {"parts": [{
                        "functionCall": {"name": name, "args": {}}
                    }]},
                    "finishReason": (index == 1).then_some("STOP")
                }]
            }))
            .expect("response");
            assert!(emit_gemini_response_chunk(
                response,
                &tx,
                &mut tool_call_state,
                &mut saw_finish_reason,
            )
            .await
            .expect("emit response"));
        }

        let first = rx.recv().await.unwrap().unwrap().tool_call_delta.unwrap();
        let second = rx.recv().await.unwrap().unwrap().tool_call_delta.unwrap();
        assert_eq!(first.id, "call_0");
        assert_eq!(second.id, "call_1");
        assert_ne!(first.name, second.name);
    }

    #[tokio::test]
    async fn test_ambiguous_missing_id_snapshots_trigger_non_streaming_retry() {
        let (tx, _rx) = mpsc::channel(4);
        let mut tool_call_state = GeminiToolCallStreamState::default();
        let mut saw_finish_reason = false;
        for (index, args) in [
            serde_json::json!({"path": "a"}),
            serde_json::json!({"path": "ab"}),
        ]
        .into_iter()
        .enumerate()
        {
            let response: GeminiResponse = serde_json::from_value(serde_json::json!({
                "candidates": [{
                    "content": {"parts": [{
                        "functionCall": {"name": "read_file", "args": args}
                    }]},
                    "finishReason": (index == 1).then_some("STOP")
                }]
            }))
            .expect("response");
            let result = emit_gemini_response_chunk(
                response,
                &tx,
                &mut tool_call_state,
                &mut saw_finish_reason,
            )
            .await;
            if index == 0 {
                assert!(result.expect("first snapshot"));
            } else {
                let error = result.expect_err("changed id-less snapshot must be ambiguous");
                assert!(matches!(error, CoreError::StreamIncomplete(_)));
                assert!(error.to_string().contains("ambiguous id-less"));
            }
        }
    }

    #[tokio::test]
    async fn test_identical_missing_id_calls_in_one_candidate_remain_distinct() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut tool_call_state = GeminiToolCallStreamState::default();
        let mut saw_finish_reason = false;
        let response: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {"parts": [
                    {"functionCall": {"name": "read_file", "args": {"path": "same"}}},
                    {"functionCall": {"name": "read_file", "args": {"path": "same"}}}
                ]},
                "finishReason": "STOP"
            }]
        }))
        .expect("response");

        assert!(emit_gemini_response_chunk(
            response,
            &tx,
            &mut tool_call_state,
            &mut saw_finish_reason,
        )
        .await
        .expect("emit response"));

        let first = rx.recv().await.unwrap().unwrap().tool_call_delta.unwrap();
        let second = rx.recv().await.unwrap().unwrap().tool_call_delta.unwrap();
        assert_eq!(first.id, "call_0");
        assert_eq!(second.id, "call_1");
        assert_eq!(first.name, second.name);
        assert_eq!(first.arguments_delta, second.arguments_delta);
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
    fn test_extract_response_maps_gemini_cached_content_tokens() {
        let resp: GeminiResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{ "text": "ok" }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "cachedContentTokenCount": 40,
                "candidatesTokenCount": 12,
                "totalTokenCount": 112,
                "thoughtsTokenCount": 0
                ,"toolUsePromptTokenCount": 18
            }
        }))
        .expect("response");

        let (_content, _tool_calls, _finish_reason, usage, _thinking) =
            extract_response(&resp).expect("extract response");

        assert_eq!(usage.prompt_tokens, 118);
        assert_eq!(usage.total_tokens, 130);
        assert_eq!(usage.tool_prompt_tokens, Some(18));
        assert_eq!(usage.cache_read_tokens, Some(40));
        assert_eq!(usage.cache_miss_tokens, None);
        assert_eq!(usage.cache_creation_tokens, None);
    }

    #[test]
    fn test_extract_response_allows_absent_gemini_cached_content_tokens() {
        let resp: GeminiResponse = serde_json::from_value(serde_json::json!({
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 12,
                "totalTokenCount": 112
            }
        }))
        .expect("response");

        let (_content, _tool_calls, _finish_reason, usage, _thinking) =
            extract_response(&resp).expect("extract response");

        assert_eq!(usage.cache_read_tokens, None);
    }
}
