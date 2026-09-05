//! SSE (Server-Sent Events) stream parser for OpenAI-compatible APIs.

use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use super::{
    next_stream_item_with_idle_timeout, FinishReason, StreamChunk, ToolCallArgumentsDelta,
    ToolCallDelta, Usage, DEFAULT_STREAM_IDLE_TIMEOUT,
};
use crate::error::CoreError;

const TERMINAL_SSE_TAIL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

// ---------------------------------------------------------------------------
// SSE JSON wire types (OpenAI streaming format)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct SseChunk {
    choices: Option<Vec<SseChoice>>,
    usage: Option<SseUsage>,
}

#[derive(serde::Deserialize)]
struct SseChoice {
    #[serde(default)]
    delta: SseDelta,
    #[serde(default)]
    message: Option<SseDelta>,
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct SseDelta {
    content: Option<serde_json::Value>,
    tool_calls: Option<Vec<SseToolCallDelta>>,
    #[serde(default, alias = "reasoningContent")]
    reasoning_content: Option<serde_json::Value>,
    #[serde(
        default,
        alias = "reasoningContentDelta",
        alias = "reasoning_content_delta"
    )]
    reasoning_content_delta: Option<serde_json::Value>,
    #[serde(default)]
    reasoning: Option<serde_json::Value>,
    #[serde(default, alias = "reasoningDelta", alias = "reasoning_delta")]
    reasoning_delta: Option<serde_json::Value>,
    #[serde(default, alias = "reasoningDetails", alias = "reasoning_details")]
    reasoning_details: Option<serde_json::Value>,
    #[serde(default, alias = "thinkingContent", alias = "thinking_content")]
    thinking: Option<serde_json::Value>,
    #[serde(default, alias = "reasoningText", alias = "reasoning_text")]
    reasoning_text: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct SseToolCallDelta {
    id: Option<String>,
    function: Option<SseFunctionDelta>,
    index: Option<u32>,
}

#[derive(serde::Deserialize)]
struct SseFunctionDelta {
    name: Option<String>,
    arguments: Option<SseArgumentsDelta>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SseArgumentsDelta {
    Text(String),
    Json(serde_json::Value),
}

impl SseArgumentsDelta {
    fn to_tool_arguments_delta(&self) -> ToolCallArgumentsDelta {
        match self {
            SseArgumentsDelta::Text(text) => text.clone().into(),
            SseArgumentsDelta::Json(value) => ToolCallArgumentsDelta::snapshot(
                serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()),
            ),
        }
    }
}

#[derive(serde::Deserialize)]
struct SseUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u32>,
    #[serde(default)]
    prompt_cache_miss_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens_details: Option<SseCompletionTokensDetails>,
    #[serde(default)]
    prompt_tokens_details: Option<SsePromptTokensDetails>,
}

#[derive(serde::Deserialize)]
struct SseCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SseCacheCreationUsage {
    Tokens(u32),
    Details(SseCacheCreationDetails),
}

impl SseCacheCreationUsage {
    fn input_tokens(&self) -> Option<u32> {
        match self {
            Self::Tokens(tokens) => Some(*tokens),
            Self::Details(details) => details.cache_creation_input_tokens,
        }
    }
}

#[derive(serde::Deserialize)]
struct SseCacheCreationDetails {
    #[serde(
        default,
        alias = "cache_write_input_tokens",
        alias = "cache_creation_tokens",
        alias = "ephemeral_5m_input_tokens"
    )]
    cache_creation_input_tokens: Option<u32>,
}

#[derive(Default)]
struct SsePromptTokensDetails {
    cached_tokens: Option<u32>,
    cache_creation: Option<SseCacheCreationUsage>,
}

impl SsePromptTokensDetails {
    fn cache_creation_tokens(&self) -> Option<u32> {
        self.cache_creation
            .as_ref()
            .and_then(SseCacheCreationUsage::input_tokens)
    }
}

impl<'de> serde::Deserialize<'de> for SsePromptTokensDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let Some(object) = value.as_object() else {
            return Ok(Self::default());
        };

        let cached_tokens = object
            .get("cached_tokens")
            .or_else(|| object.get("cache_read_input_tokens"))
            .or_else(|| object.get("cachedTokens"))
            .and_then(value_as_u32);

        let cache_creation = object
            .get("cache_creation")
            .and_then(deserialize_cache_creation_usage)
            .or_else(|| {
                first_u32_field(
                    object,
                    &[
                        "cache_creation_input_tokens",
                        "cache_write_input_tokens",
                        "cache_write_tokens",
                    ],
                )
                .map(SseCacheCreationUsage::Tokens)
            });

        Ok(Self {
            cached_tokens,
            cache_creation,
        })
    }
}

fn deserialize_cache_creation_usage(value: &serde_json::Value) -> Option<SseCacheCreationUsage> {
    if let Some(tokens) = value_as_u32(value) {
        return Some(SseCacheCreationUsage::Tokens(tokens));
    }

    let object = value.as_object()?;
    first_u32_field(
        object,
        &[
            "cache_creation_input_tokens",
            "cache_write_input_tokens",
            "cache_creation_tokens",
            "ephemeral_5m_input_tokens",
        ],
    )
    .map(SseCacheCreationUsage::Tokens)
}

fn first_u32_field(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<u32> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_as_u32))
}

fn value_as_u32(value: &serde_json::Value) -> Option<u32> {
    value.as_u64().and_then(|tokens| u32::try_from(tokens).ok())
}

fn provider_observability_fragment(value: &serde_json::Value) -> Option<serde_json::Value> {
    let object = value.as_object()?;
    let mut fragment = serde_json::Map::new();
    for (source, target) in [
        ("usage", "usage"),
        ("id", "generationId"),
        ("model", "responseModel"),
        ("openrouter_metadata", "openrouterMetadata"),
    ] {
        if let Some(value) = object.get(source) {
            fragment.insert(target.to_string(), value.clone());
        }
    }
    (!fragment.is_empty()).then_some(serde_json::Value::Object(fragment))
}

// ---------------------------------------------------------------------------
// Mapping helpers
// ---------------------------------------------------------------------------

fn parse_finish_reason(s: &str) -> Result<FinishReason, CoreError> {
    match s {
        "stop" => Ok(FinishReason::Stop),
        "length" => Ok(FinishReason::Length),
        "tool_calls" | "function_call" => Ok(FinishReason::ToolCalls),
        "content_filter" => Ok(FinishReason::ContentFilter),
        "insufficient_system_resource" => Err(CoreError::StreamIncomplete(
            "DeepSeek interrupted the stream because capacity was unavailable during a peak period"
                .to_string(),
        )),
        other => Ok(FinishReason::Unknown(other.to_string())),
    }
}

fn map_tool_call_delta(tc: &SseToolCallDelta) -> ToolCallDelta {
    ToolCallDelta {
        id: tc.id.clone().unwrap_or_default(),
        name: tc.function.as_ref().and_then(|f| f.name.clone()),
        arguments_delta: tc
            .function
            .as_ref()
            .and_then(|f| {
                f.arguments
                    .as_ref()
                    .map(SseArgumentsDelta::to_tool_arguments_delta)
            })
            .unwrap_or_default(),
        index: tc.index,
        thought_signature: None,
    }
}

fn json_value_to_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(json_value_to_text)
                .collect::<Vec<_>>()
                .join("");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        serde_json::Value::Object(map) => {
            for key in [
                "reasoning_content",
                "reasoningContent",
                "thinking",
                "thinking_content",
                "thinkingContent",
                "reasoning_text",
                "reasoningText",
                "reasoning_delta",
                "reasoningDelta",
                "reasoning_details",
                "reasoningDetails",
                "reasoning_content_delta",
                "reasoningContentDelta",
                "summary_text",
                "summaryText",
                "delta",
                "text_delta",
                "textDelta",
                "text",
                "content",
                "output_text",
                "summary",
            ] {
                if let Some(v) = map.get(key) {
                    if let Some(text) = json_value_to_text(v) {
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn append_structured_content(value: &serde_json::Value, text: &mut String, reasoning: &mut String) {
    match value {
        serde_json::Value::String(value) => text.push_str(value),
        serde_json::Value::Array(values) => {
            for value in values {
                append_structured_content(value, text, reasoning);
            }
        }
        serde_json::Value::Object(object) => {
            let content_type = object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(content_type.as_str(), "thinking" | "reasoning") {
                if let Some(value) = object
                    .get("thinking")
                    .or_else(|| object.get("reasoning"))
                    .or_else(|| object.get("content"))
                    .or_else(|| object.get("text"))
                    .and_then(json_value_to_text)
                {
                    reasoning.push_str(&value);
                }
                return;
            }

            if matches!(content_type.as_str(), "text" | "output_text") {
                if let Some(value) = object
                    .get("text")
                    .or_else(|| object.get("content"))
                    .and_then(json_value_to_text)
                {
                    text.push_str(&value);
                }
                return;
            }

            if let Some(value) = object
                .get("thinking")
                .or_else(|| object.get("reasoning"))
                .and_then(json_value_to_text)
            {
                reasoning.push_str(&value);
            }
            if let Some(value) = object
                .get("text")
                .or_else(|| object.get("content"))
                .and_then(json_value_to_text)
            {
                text.push_str(&value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

pub(crate) fn partition_openai_content(value: &serde_json::Value) -> (String, Option<String>) {
    let mut text = String::new();
    let mut reasoning = String::new();
    append_structured_content(value, &mut text, &mut reasoning);
    (text, (!reasoning.is_empty()).then_some(reasoning))
}

pub(crate) fn partition_complete_think_tags(text: &str) -> (String, Option<String>) {
    let mut in_think_block = false;
    let mut tag_buffer = String::new();
    split_think_tags(text, &mut in_think_block, &mut tag_buffer)
}

fn extract_reasoning_delta(delta: &SseDelta) -> Option<String> {
    for v in [
        delta.reasoning_content.as_ref(),
        delta.reasoning_content_delta.as_ref(),
        delta.reasoning.as_ref(),
        delta.reasoning_delta.as_ref(),
        delta.reasoning_details.as_ref(),
        delta.thinking.as_ref(),
        delta.reasoning_text.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(text) = json_value_to_text(v) {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn extract_reasoning_from_choice(choice: &SseChoice) -> Option<String> {
    extract_reasoning_delta(&choice.delta)
        .or_else(|| choice.message.as_ref().and_then(extract_reasoning_delta))
        .or_else(|| {
            choice
                .delta
                .content
                .as_ref()
                .and_then(|content| partition_openai_content(content).1)
        })
        .or_else(|| {
            choice
                .message
                .as_ref()
                .and_then(|message| message.content.as_ref())
                .and_then(|content| partition_openai_content(content).1)
        })
}

fn extract_text_delta_from_choice(choice: &SseChoice) -> String {
    choice
        .delta
        .content
        .as_ref()
        .map(|content| partition_openai_content(content).0)
        .or_else(|| {
            choice
                .message
                .as_ref()
                .and_then(|message| message.content.as_ref())
                .map(|content| partition_openai_content(content).0)
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Thinking-token tag definitions
// ---------------------------------------------------------------------------

const THINK_OPEN_TAGS: &[&str] = &["<think>", "<|begin_of_thinking|>", "<|startofthought|>"];

const THINK_CLOSE_TAGS: &[&str] = &["</think>", "<|end_of_thinking|>", "<|endofthought|>"];

/// Find the earliest occurrence of any tag in `haystack`.
/// Returns `(byte_position, tag_byte_length)` or `None`.
fn find_earliest_tag(haystack: &str, tags: &[&str]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for tag in tags {
        if let Some(pos) = haystack.find(tag) {
            if best.is_none_or(|(bp, _)| pos < bp) {
                best = Some((pos, tag.len()));
            }
        }
    }
    best
}

/// Length of the longest suffix of `haystack` that equals a *proper* prefix of
/// one of the `tags`.  Returns 0 when there is no partial match.
fn partial_tag_suffix_len(haystack: &str, tags: &[&str]) -> usize {
    if haystack.is_empty() {
        return 0;
    }
    let mut max_len = 0usize;
    for tag in tags {
        let max_check = tag.len().saturating_sub(1).min(haystack.len());
        for len in (1..=max_check).rev() {
            let start = haystack.len() - len;
            if !haystack.is_char_boundary(start) {
                continue;
            }
            let suffix = &haystack[start..];
            if tag.starts_with(suffix) {
                if len > max_len {
                    max_len = len;
                }
                break;
            }
        }
    }
    max_len
}

/// Split provider content into visible text and thinking-tag reasoning text.
///
/// Handles `<think>…</think>`, `<|begin_of_thinking|>…<|end_of_thinking|>`,
/// and `<|startofthought|>…<|endofthought|>` formats, including partial tags
/// split across SSE chunks.
fn split_think_tags(
    raw_delta: &str,
    in_think_block: &mut bool,
    tag_buffer: &mut String,
) -> (String, Option<String>) {
    if raw_delta.is_empty() && tag_buffer.is_empty() {
        return (String::new(), None);
    }

    if !raw_delta.is_empty() {
        tag_buffer.push_str(raw_delta);
    }

    let mut visible = String::new();
    let mut thinking = String::new();

    loop {
        if *in_think_block {
            if let Some((end_pos, tag_len)) = find_earliest_tag(tag_buffer, THINK_CLOSE_TAGS) {
                let think_part = &tag_buffer[..end_pos];
                if !think_part.is_empty() {
                    thinking.push_str(think_part);
                }
                *tag_buffer = tag_buffer[end_pos + tag_len..].to_string();
                *in_think_block = false;
            } else {
                // Hold back any suffix that could be the start of a close tag.
                let hold = partial_tag_suffix_len(tag_buffer, THINK_CLOSE_TAGS);
                let flush = tag_buffer.len() - hold;
                if flush > 0 {
                    thinking.push_str(&tag_buffer[..flush]);
                }
                if hold > 0 {
                    *tag_buffer = tag_buffer[flush..].to_string();
                } else {
                    tag_buffer.clear();
                }
                break;
            }
        } else if let Some((start_pos, tag_len)) = find_earliest_tag(tag_buffer, THINK_OPEN_TAGS) {
            let before = &tag_buffer[..start_pos];
            if !before.is_empty() {
                visible.push_str(before);
            }
            *tag_buffer = tag_buffer[start_pos + tag_len..].to_string();
            *in_think_block = true;
        } else {
            // Hold back any suffix that could be the start of an open tag.
            let hold = partial_tag_suffix_len(tag_buffer, THINK_OPEN_TAGS);
            let flush = tag_buffer.len() - hold;
            if flush > 0 {
                visible.push_str(&tag_buffer[..flush]);
            }
            if hold > 0 {
                *tag_buffer = tag_buffer[flush..].to_string();
            } else {
                tag_buffer.clear();
            }
            break;
        }
    }

    let thinking = if thinking.is_empty() {
        None
    } else {
        Some(thinking)
    };

    (visible, thinking)
}

fn decode_sse_line(line_bytes: &[u8]) -> String {
    match std::str::from_utf8(line_bytes) {
        Ok(line) => line.to_string(),
        Err(e) => {
            warn!(
                "Invalid UTF-8 in complete SSE line ({} bytes): {e} — decoding lossy",
                line_bytes.len()
            );
            String::from_utf8_lossy(line_bytes).into_owned()
        }
    }
}

fn drain_complete_sse_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0usize;

    while let Some(relative_newline) = buffer[start..].iter().position(|byte| *byte == b'\n') {
        let newline_pos = start + relative_newline;
        let mut line_bytes = &buffer[start..newline_pos];
        if line_bytes.ends_with(b"\r") {
            line_bytes = &line_bytes[..line_bytes.len().saturating_sub(1)];
        }
        lines.push(decode_sse_line(line_bytes));
        start = newline_pos + 1;
    }

    if start > 0 {
        buffer.drain(..start);
    }

    lines
}

async fn process_sse_line(
    line: String,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    in_think_block: &mut bool,
    think_tag_buffer: &mut String,
    terminal_finish_seen: &mut bool,
) -> Result<bool, CoreError> {
    if line.is_empty() {
        return Ok(false);
    }

    // Only process `data:` lines; ignore `event:`, `id:`, `retry:`, etc.
    let Some(data) = line
        .strip_prefix("data: ")
        .or_else(|| line.strip_prefix("data:"))
    else {
        return Ok(false);
    };

    let data = data.trim();

    // Stream termination signal.
    if data == "[DONE]" {
        debug!("SSE [DONE] received");
        // Flush any held-back buffer content at stream end.
        if !think_tag_buffer.is_empty() {
            let tail = std::mem::take(think_tag_buffer);
            if *in_think_block {
                let _ = tx
                    .send(Ok(StreamChunk {
                        delta: String::new(),
                        tool_call_delta: None,
                        finish_reason: None,
                        usage: None,
                        thinking_delta: Some(tail),
                    }))
                    .await;
            } else {
                let _ = tx
                    .send(Ok(StreamChunk {
                        delta: tail,
                        tool_call_delta: None,
                        finish_reason: None,
                        usage: None,
                        thinking_delta: None,
                    }))
                    .await;
            }
        }
        return Ok(true);
    }

    // Parse JSON and send through channel.
    let provider_raw = serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .as_ref()
        .and_then(provider_observability_fragment);
    match serde_json::from_str::<SseChunk>(data) {
        Ok(sse) => {
            let choice = sse.choices.as_ref().and_then(|c| c.first());
            let raw_delta = choice
                .map(extract_text_delta_from_choice)
                .unwrap_or_default();
            let (delta, think_from_tags) =
                split_think_tags(&raw_delta, in_think_block, think_tag_buffer);
            let finish_reason = choice
                .and_then(|c| c.finish_reason.as_deref())
                .map(parse_finish_reason)
                .transpose()?;
            *terminal_finish_seen |= finish_reason.is_some();
            let usage = sse.usage.map(|u| {
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
                        .and_then(SsePromptTokensDetails::cache_creation_tokens),
                    provider_raw: provider_raw.clone(),
                }
            });

            // Emit provider-specific reasoning/thinking deltas if present.
            let mut thinking_delta = choice
                .and_then(extract_reasoning_from_choice)
                .filter(|s| !s.is_empty());
            if let Some(tag_thinking) = think_from_tags {
                match &mut thinking_delta {
                    Some(existing) => {
                        if existing != &tag_thinking {
                            existing.push_str(&tag_thinking);
                        }
                    }
                    None => thinking_delta = Some(tag_thinking),
                }
            }

            // Emit text/finish/usage metadata as one chunk.
            #[allow(clippy::collapsible_if)]
            if !delta.is_empty()
                || finish_reason.is_some()
                || usage.is_some()
                || thinking_delta.is_some()
            {
                if tx
                    .send(Ok(StreamChunk {
                        delta,
                        tool_call_delta: None,
                        finish_reason,
                        usage,
                        thinking_delta,
                    }))
                    .await
                    .is_err()
                {
                    return Ok(true);
                }
            }

            // Emit each tool call delta separately so multiple tool calls
            // in one SSE frame are preserved.
            if let Some(tool_calls) = choice.and_then(|c| {
                c.delta
                    .tool_calls
                    .as_ref()
                    .or_else(|| c.message.as_ref().and_then(|m| m.tool_calls.as_ref()))
            }) {
                for tc in tool_calls {
                    if tx
                        .send(Ok(StreamChunk {
                            delta: String::new(),
                            tool_call_delta: Some(map_tool_call_delta(tc)),
                            finish_reason: None,
                            usage: None,
                            thinking_delta: None,
                        }))
                        .await
                        .is_err()
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Err(e) => {
            // Send parse error through channel but continue processing.
            warn!("SSE JSON parse error: {e}, data: {data}");
            if tx
                .send(Err(CoreError::Llm(format!("SSE JSON parse error: {e}"))))
                .await
                .is_err()
            {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

async fn process_sse_event_data_lines(
    event_data_lines: &mut Vec<String>,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    in_think_block: &mut bool,
    think_tag_buffer: &mut String,
    terminal_finish_seen: &mut bool,
) -> Result<bool, CoreError> {
    if event_data_lines.is_empty() {
        return Ok(false);
    }

    let data = event_data_lines.join("\n");
    event_data_lines.clear();
    process_sse_line(
        format!("data: {data}"),
        tx,
        in_think_block,
        think_tag_buffer,
        terminal_finish_seen,
    )
    .await
}

async fn collect_or_dispatch_sse_line(
    line: String,
    event_data_lines: &mut Vec<String>,
    tx: &mpsc::Sender<Result<StreamChunk, CoreError>>,
    in_think_block: &mut bool,
    think_tag_buffer: &mut String,
    terminal_finish_seen: &mut bool,
) -> Result<bool, CoreError> {
    if line.is_empty() {
        return process_sse_event_data_lines(
            event_data_lines,
            tx,
            in_think_block,
            think_tag_buffer,
            terminal_finish_seen,
        )
        .await;
    }

    let Some(data) = line
        .strip_prefix("data: ")
        .or_else(|| line.strip_prefix("data:"))
    else {
        return Ok(false);
    };

    let trimmed = data.trim_start();
    if !event_data_lines.is_empty()
        && (trimmed.starts_with('{') || trimmed == "[DONE]")
        && process_sse_event_data_lines(
            event_data_lines,
            tx,
            in_think_block,
            think_tag_buffer,
            terminal_finish_seen,
        )
        .await?
    {
        return Ok(true);
    }

    event_data_lines.push(data.to_string());
    Ok(false)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an SSE stream from an HTTP response and send chunks to the channel.
///
/// Handles `data: [DONE]` termination.
/// Each SSE line starts with `data: ` and contains a JSON object.
pub async fn parse_sse_stream(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, CoreError>>,
) -> Result<(), CoreError> {
    parse_sse_stream_with_idle_timeout(response, tx, DEFAULT_STREAM_IDLE_TIMEOUT).await
}

pub async fn parse_sse_stream_with_idle_timeout(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamChunk, CoreError>>,
    stream_idle_timeout: std::time::Duration,
) -> Result<(), CoreError> {
    let mut byte_stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut in_think_block = false;
    let mut think_tag_buffer = String::new();
    let mut event_data_lines: Vec<String> = Vec::new();
    let mut first_chunk = true;
    let mut terminal_finish_seen = false;

    loop {
        let idle_timeout = if terminal_finish_seen {
            stream_idle_timeout.min(TERMINAL_SSE_TAIL_GRACE)
        } else {
            stream_idle_timeout
        };
        let Some(chunk_result) = (match next_stream_item_with_idle_timeout(
            &mut byte_stream,
            idle_timeout,
            "OpenAI SSE stream",
        )
        .await
        {
            Ok(next) => next,
            Err(error)
                if terminal_finish_seen
                    && matches!(
                        error,
                        CoreError::StreamIncomplete(_) | CoreError::TransientLlm(_)
                    ) =>
            {
                warn!("Ignoring transport shutdown after a terminal finish_reason: {error}");
                break;
            }
            Err(error) => return Err(error),
        }) else {
            break;
        };
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(error) => {
                error!("Stream read error: {error}");
                let detail = super::transport::describe_http_error(&error);
                let message = detail.to_ascii_lowercase();
                let error = if message.contains("decoding response body")
                    || message.contains("connection")
                    || message.contains("closed")
                    || message.contains("reset")
                    || message.contains("broken pipe")
                    || message.contains("incompleted")
                    || message.contains("eof")
                {
                    CoreError::TransientLlm(format!("stream interrupted: {detail}"))
                } else {
                    CoreError::Llm(format!("Stream read error: {detail}"))
                };
                if terminal_finish_seen && matches!(error, CoreError::TransientLlm(_)) {
                    warn!("Ignoring transport shutdown after a terminal finish_reason: {error}");
                    break;
                }
                return Err(error);
            }
        };
        if first_chunk {
            debug!("First SSE chunk received");
            first_chunk = false;
        }

        buffer.extend_from_slice(&chunk);
        for line in drain_complete_sse_lines(&mut buffer) {
            if collect_or_dispatch_sse_line(
                line,
                &mut event_data_lines,
                &tx,
                &mut in_think_block,
                &mut think_tag_buffer,
                &mut terminal_finish_seen,
            )
            .await?
            {
                return Ok(());
            }
        }
    }

    if !buffer.is_empty() {
        let line = decode_sse_line(&buffer);
        if collect_or_dispatch_sse_line(
            line,
            &mut event_data_lines,
            &tx,
            &mut in_think_block,
            &mut think_tag_buffer,
            &mut terminal_finish_seen,
        )
        .await?
        {
            return Ok(());
        }
    }

    if process_sse_event_data_lines(
        &mut event_data_lines,
        &tx,
        &mut in_think_block,
        &mut think_tag_buffer,
        &mut terminal_finish_seen,
    )
    .await?
    {
        return Ok(());
    }

    // A terminal finish_reason is the semantic boundary. A few compatible
    // servers close cleanly (or with a harmless HTTP tail error) without the
    // optional transport sentinel; replaying that completed sample can repeat
    // tool calls and visible output.
    if terminal_finish_seen {
        if !think_tag_buffer.is_empty() {
            let tail = std::mem::take(&mut think_tag_buffer);
            let chunk = if in_think_block {
                StreamChunk {
                    delta: String::new(),
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: Some(tail),
                }
            } else {
                StreamChunk {
                    delta: tail,
                    tool_call_delta: None,
                    finish_reason: None,
                    usage: None,
                    thinking_delta: None,
                }
            };
            let _ = tx.send(Ok(chunk)).await;
        }
        return Ok(());
    }

    // Stream ended without either semantic or transport terminal evidence.
    warn!("Stream ended without [DONE] marker");
    Err(CoreError::StreamIncomplete(
        "stream ended without [DONE] marker".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_terminal_chat_sse_without_done(
        listener: tokio::net::TcpListener,
        truncated_tail: bool,
    ) -> std::io::Result<()> {
        let (mut socket, _) = listener.accept().await?;
        let mut request = [0u8; 2048];
        let _ = socket.read(&mut request).await?;
        let body = br#"data: {"choices":[{"delta":{"content":"finished"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}

"#;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() + usize::from(truncated_tail)
                )
                .as_bytes(),
            )
            .await?;
        socket.write_all(body).await?;
        socket.shutdown().await
    }

    #[tokio::test]
    async fn terminal_finish_reason_accepts_clean_eof_without_done_sentinel() {
        assert_terminal_finish_survives_transport_end(false).await;
    }

    #[tokio::test]
    async fn terminal_finish_reason_survives_truncated_http_body() {
        assert_terminal_finish_survives_transport_end(true).await;
    }

    async fn assert_terminal_finish_survives_transport_end(truncated_tail: bool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(serve_terminal_chat_sse_without_done(
            listener,
            truncated_tail,
        ));
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/chat/completions"))
            .send()
            .await
            .expect("fixture response");
        let (tx, mut rx) = mpsc::channel(4);

        parse_sse_stream_with_idle_timeout(response, tx, std::time::Duration::from_secs(1))
            .await
            .expect("a complete finish_reason is a safe terminal boundary");
        server.await.expect("fixture task").expect("fixture server");

        let chunk = rx
            .recv()
            .await
            .expect("terminal chunk")
            .expect("valid terminal chunk");
        assert_eq!(chunk.delta, "finished");
        assert_eq!(chunk.finish_reason, Some(FinishReason::Stop));
    }

    #[tokio::test]
    async fn deepseek_reasoning_only_length_chunk_keeps_answer_empty() {
        let (tx, mut rx) = mpsc::channel(2);
        let mut in_think_block = false;
        let mut think_tag_buffer = String::new();
        let mut terminal_finish_seen = false;
        let line = r#"data: {"choices":[{"delta":{"content":null,"reasoning_content":"raw internal reasoning"},"finish_reason":"length"}]}"#;

        let done = process_sse_line(
            line.to_string(),
            &tx,
            &mut in_think_block,
            &mut think_tag_buffer,
            &mut terminal_finish_seen,
        )
        .await
        .expect("DeepSeek-compatible SSE chunk should parse");
        assert!(!done);

        let chunk = rx
            .recv()
            .await
            .expect("chunk should be emitted")
            .expect("chunk should be valid");
        assert!(chunk.delta.is_empty());
        assert_eq!(
            chunk.thinking_delta.as_deref(),
            Some("raw internal reasoning")
        );
        assert_eq!(chunk.finish_reason, Some(FinishReason::Length));
    }

    #[tokio::test]
    async fn deepseek_capacity_finish_is_a_recoverable_stream_interruption() {
        let (tx, _rx) = mpsc::channel(2);
        let mut in_think_block = false;
        let mut think_tag_buffer = String::new();
        let mut terminal_finish_seen = false;
        let line =
            r#"data: {"choices":[{"delta":{},"finish_reason":"insufficient_system_resource"}]}"#;

        let error = process_sse_line(
            line.to_string(),
            &tx,
            &mut in_think_block,
            &mut think_tag_buffer,
            &mut terminal_finish_seen,
        )
        .await
        .expect_err("capacity interruption must enter stream recovery");

        assert!(matches!(
            error,
            CoreError::StreamIncomplete(ref message)
                if message.contains("DeepSeek") && message.contains("capacity")
        ));
    }

    #[test]
    fn extracts_reasoning_from_delta_reasoning_content() {
        let choice: SseChoice = serde_json::from_value(serde_json::json!({
            "delta": {
                "reasoning_content": "thinking from delta"
            },
            "finish_reason": null
        }))
        .expect("deserialize choice");

        assert_eq!(
            extract_reasoning_from_choice(&choice).as_deref(),
            Some("thinking from delta")
        );
    }

    #[test]
    fn extracts_reasoning_from_message_fallback() {
        let choice: SseChoice = serde_json::from_value(serde_json::json!({
            "delta": {},
            "message": {
                "reasoning_content": "thinking from message"
            },
            "finish_reason": "stop"
        }))
        .expect("deserialize choice");

        assert_eq!(
            extract_reasoning_from_choice(&choice).as_deref(),
            Some("thinking from message")
        );
    }

    #[test]
    fn extracts_reasoning_from_nested_delta_key() {
        let choice: SseChoice = serde_json::from_value(serde_json::json!({
            "delta": {
                "reasoning": {
                    "delta": "partial reasoning"
                }
            }
        }))
        .expect("deserialize choice");

        assert_eq!(
            extract_reasoning_from_choice(&choice).as_deref(),
            Some("partial reasoning")
        );
    }

    #[test]
    fn extracts_reasoning_from_openrouter_reasoning_details() {
        let choice: SseChoice = serde_json::from_value(serde_json::json!({
            "delta": {
                "reasoning_details": [
                    { "type": "reasoning.text", "text": "first " },
                    { "type": "reasoning.summary", "summary_text": "second" }
                ]
            }
        }))
        .expect("deserialize choice");

        assert_eq!(
            extract_reasoning_from_choice(&choice).as_deref(),
            Some("first second")
        );
    }

    #[test]
    fn extracts_text_from_message_when_delta_content_missing() {
        let choice: SseChoice = serde_json::from_value(serde_json::json!({
            "delta": {},
            "message": {
                "content": "assistant output"
            }
        }))
        .expect("deserialize choice");

        assert_eq!(extract_text_delta_from_choice(&choice), "assistant output");
    }

    #[test]
    fn partitions_mistral_thinking_chunks_from_visible_text() {
        let choice: SseChoice = serde_json::from_value(serde_json::json!({
            "delta": {
                "content": [
                    {
                        "type": "thinking",
                        "thinking": [{"type": "text", "text": "work it out"}]
                    },
                    {"type": "text", "text": "final answer"}
                ]
            }
        }))
        .expect("deserialize choice");

        assert_eq!(extract_text_delta_from_choice(&choice), "final answer");
        assert_eq!(
            extract_reasoning_from_choice(&choice).as_deref(),
            Some("work it out")
        );
    }

    #[test]
    fn tool_call_delta_accepts_json_object_arguments() {
        let tool_call: SseToolCallDelta = serde_json::from_value(serde_json::json!({
            "id": "call_1",
            "index": 0,
            "function": {
                "name": "lookup",
                "arguments": { "q": "x" }
            }
        }))
        .expect("deserialize tool call delta");

        let delta = map_tool_call_delta(&tool_call);

        assert_eq!(delta.id, "call_1");
        assert_eq!(delta.name.as_deref(), Some("lookup"));
        assert_eq!(delta.arguments_delta, "{\"q\":\"x\"}");
        assert!(delta.arguments_delta.is_snapshot());
    }

    #[test]
    fn stream_usage_maps_deepseek_top_level_cache_hits() {
        let usage: SseUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_cache_hit_tokens": 48,
            "prompt_cache_miss_tokens": 52
        }))
        .expect("usage");

        let prompt_details = usage.prompt_tokens_details;
        let cache_read_tokens = super::super::prompt_cache::openai_compatible_cache_read_tokens(
            prompt_details.as_ref().and_then(|d| d.cached_tokens),
            usage.prompt_cache_hit_tokens,
        );

        assert_eq!(cache_read_tokens, Some(48));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(52));
    }

    #[test]
    fn stream_usage_maps_qwen_nested_cache_creation() {
        let usage: SseUsage = serde_json::from_value(serde_json::json!({
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
        .expect("usage");

        let details = usage.prompt_tokens_details.expect("details");
        assert_eq!(details.cached_tokens, Some(64));
        assert_eq!(details.cache_creation_tokens(), Some(24));
    }

    #[test]
    fn stream_usage_accepts_qwen_duplicate_cache_creation_aliases() {
        let usage: SseUsage = serde_json::from_str(
            r#"{
                "prompt_tokens": 120,
                "completion_tokens": 20,
                "total_tokens": 140,
                "prompt_tokens_details": {
                    "cached_tokens": 64,
                    "cache_creation": 12,
                    "cache_creation_input_tokens": 24
                }
            }"#,
        )
        .expect("usage");

        let details = usage.prompt_tokens_details.expect("details");
        assert_eq!(details.cached_tokens, Some(64));
        assert_eq!(details.cache_creation_tokens(), Some(12));
    }

    // -- split_think_tags tests -------------------------------------------

    #[test]
    fn think_tags_basic() {
        let mut in_block = false;
        let mut buf = String::new();
        let (vis, think) = split_think_tags(
            "hello<think>reasoning</think>world",
            &mut in_block,
            &mut buf,
        );
        assert_eq!(vis, "helloworld");
        assert_eq!(think.as_deref(), Some("reasoning"));
        assert!(!in_block);
    }

    #[test]
    fn begin_of_thinking_tags() {
        let mut in_block = false;
        let mut buf = String::new();
        let (vis, think) = split_think_tags(
            "hello<|begin_of_thinking|>deep thought<|end_of_thinking|>world",
            &mut in_block,
            &mut buf,
        );
        assert_eq!(vis, "helloworld");
        assert_eq!(think.as_deref(), Some("deep thought"));
        assert!(!in_block);
    }

    #[test]
    fn startofthought_tags() {
        let mut in_block = false;
        let mut buf = String::new();
        let (vis, think) = split_think_tags(
            "hi<|startofthought|>pondering<|endofthought|>bye",
            &mut in_block,
            &mut buf,
        );
        assert_eq!(vis, "hibye");
        assert_eq!(think.as_deref(), Some("pondering"));
    }

    #[test]
    fn partial_open_tag_across_chunks() {
        let mut in_block = false;
        let mut buf = String::new();

        // First chunk ends mid-tag.
        let (vis1, think1) = split_think_tags("hello<|begin_of_", &mut in_block, &mut buf);
        assert_eq!(vis1, "hello");
        assert!(think1.is_none());
        assert!(!in_block);
        // Buffer holds the partial tag.
        assert!(!buf.is_empty());

        // Second chunk completes the tag.
        let (vis2, think2) = split_think_tags(
            "thinking|>secret<|end_of_thinking|>world",
            &mut in_block,
            &mut buf,
        );
        assert_eq!(vis2, "world");
        assert_eq!(think2.as_deref(), Some("secret"));
        assert!(!in_block);
    }

    #[test]
    fn partial_close_tag_across_chunks() {
        let mut in_block = true;
        let mut buf = String::new();

        // First chunk ends mid-close-tag.
        let (vis1, think1) = split_think_tags("reasoning<|end_of_", &mut in_block, &mut buf);
        assert_eq!(vis1, "");
        assert_eq!(think1.as_deref(), Some("reasoning"));
        assert!(in_block);

        // Second chunk completes the close tag.
        let (vis2, think2) = split_think_tags("thinking|>visible", &mut in_block, &mut buf);
        assert_eq!(vis2, "visible");
        assert!(think2.is_none());
        assert!(!in_block);
    }

    #[test]
    fn no_tags_passes_through() {
        let mut in_block = false;
        let mut buf = String::new();
        let (vis, think) = split_think_tags("just text", &mut in_block, &mut buf);
        assert_eq!(vis, "just text");
        assert!(think.is_none());
    }

    #[test]
    fn empty_input_returns_empty() {
        let mut in_block = false;
        let mut buf = String::new();
        let (vis, think) = split_think_tags("", &mut in_block, &mut buf);
        assert_eq!(vis, "");
        assert!(think.is_none());
    }

    #[test]
    fn partial_tag_suffix_len_finds_prefix() {
        assert_eq!(partial_tag_suffix_len("hello<|begin", THINK_OPEN_TAGS), 7);
        assert_eq!(partial_tag_suffix_len("text<", THINK_OPEN_TAGS), 1);
        assert_eq!(partial_tag_suffix_len("nothing", THINK_OPEN_TAGS), 0);
        assert_eq!(partial_tag_suffix_len("x</thi", THINK_CLOSE_TAGS), 5);
    }

    #[test]
    fn cjk_text_no_panic() {
        // CJK characters are 3 bytes each; byte-level slicing must not
        // land inside a multi-byte character.
        let mut in_block = false;
        let mut buf = String::new();
        let (vis, think) =
            split_think_tags("根据<think>中文思考</think>结果", &mut in_block, &mut buf);
        assert_eq!(vis, "根据结果");
        assert_eq!(think.as_deref(), Some("中文思考"));
        assert!(!in_block);
    }

    #[test]
    fn cjk_partial_tag_no_panic() {
        // Suffix scan must skip non-char-boundary positions.
        assert_eq!(partial_tag_suffix_len("根据", THINK_OPEN_TAGS), 0);
        assert_eq!(partial_tag_suffix_len("根据<", THINK_OPEN_TAGS), 1);
        assert_eq!(partial_tag_suffix_len("根据</thin", THINK_CLOSE_TAGS), 6);
    }

    #[test]
    fn cjk_in_think_block_across_chunks() {
        let mut in_block = true;
        let mut buf = String::new();
        // First chunk: CJK reasoning with partial close tag.
        let (vis1, think1) = split_think_tags("中文推理</thi", &mut in_block, &mut buf);
        assert_eq!(vis1, "");
        assert_eq!(think1.as_deref(), Some("中文推理"));
        assert!(in_block);
        // Second chunk completes the close tag.
        let (vis2, think2) = split_think_tags("nk>可见文本", &mut in_block, &mut buf);
        assert_eq!(vis2, "可见文本");
        assert!(think2.is_none());
        assert!(!in_block);
    }

    #[test]
    fn drain_complete_sse_lines_waits_for_split_utf8_character() {
        let text = "data: 中文\n";
        let split_inside_first_cjk = text.find('中').expect("find CJK char") + 1;
        let mut buffer = Vec::new();

        buffer.extend_from_slice(&text.as_bytes()[..split_inside_first_cjk]);
        assert!(drain_complete_sse_lines(&mut buffer).is_empty());

        buffer.extend_from_slice(&text.as_bytes()[split_inside_first_cjk..]);
        let lines = drain_complete_sse_lines(&mut buffer);

        assert_eq!(lines, vec!["data: 中文".to_string()]);
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn multiline_sse_data_event_is_parsed_as_one_json_payload() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut event_data_lines = Vec::new();
        let mut in_think_block = false;
        let mut think_tag_buffer = String::new();
        let mut terminal_finish_seen = false;

        for line in [
            "data: {",
            "data: \"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]",
            "data: }",
            "",
        ] {
            let done = collect_or_dispatch_sse_line(
                line.to_string(),
                &mut event_data_lines,
                &tx,
                &mut in_think_block,
                &mut think_tag_buffer,
                &mut terminal_finish_seen,
            )
            .await
            .expect("process line");
            assert!(!done);
        }

        let chunk = rx
            .recv()
            .await
            .expect("chunk")
            .expect("parsed stream chunk");
        assert_eq!(chunk.delta, "hello");
        assert!(rx.try_recv().is_err());
    }
}
