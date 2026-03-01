//! SSE (Server-Sent Events) stream parser for OpenAI-compatible APIs.

use futures::StreamExt;
use tokio::sync::mpsc;

use super::{FinishReason, StreamChunk, ToolCallDelta, Usage};
use crate::error::CoreError;

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
    content: Option<String>,
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
    arguments: Option<String>,
}

#[derive(serde::Deserialize)]
struct SseUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    completion_tokens_details: Option<SseCompletionTokensDetails>,
}

#[derive(serde::Deserialize)]
struct SseCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

// ---------------------------------------------------------------------------
// Mapping helpers
// ---------------------------------------------------------------------------

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn map_tool_call_delta(tc: &SseToolCallDelta) -> ToolCallDelta {
    ToolCallDelta {
        id: tc.id.clone().unwrap_or_default(),
        name: tc.function.as_ref().and_then(|f| f.name.clone()),
        arguments_delta: tc
            .function
            .as_ref()
            .and_then(|f| f.arguments.clone())
            .unwrap_or_default(),
        index: tc.index,
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
                "reasoning_content_delta",
                "reasoningContentDelta",
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

fn extract_reasoning_delta(delta: &SseDelta) -> Option<String> {
    for value in [
        delta.reasoning_content.as_ref(),
        delta.reasoning_content_delta.as_ref(),
        delta.reasoning.as_ref(),
        delta.reasoning_delta.as_ref(),
        delta.thinking.as_ref(),
        delta.reasoning_text.as_ref(),
    ] {
        if let Some(v) = value {
            if let Some(text) = json_value_to_text(v) {
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

fn extract_reasoning_from_choice(choice: &SseChoice) -> Option<String> {
    extract_reasoning_delta(&choice.delta)
        .or_else(|| choice.message.as_ref().and_then(extract_reasoning_delta))
}

fn extract_text_delta_from_choice(choice: &SseChoice) -> String {
    choice
        .delta
        .content
        .clone()
        .or_else(|| choice.message.as_ref().and_then(|m| m.content.clone()))
        .unwrap_or_default()
}

/// Split provider content into visible text and `<think>...</think>` reasoning text.
///
/// Keeps parser state so tags can be detected even when split across SSE chunks.
fn split_think_tags(
    raw_delta: &str,
    in_think_block: &mut bool,
    tag_buffer: &mut String,
) -> (String, Option<String>) {
    if raw_delta.is_empty() {
        return (String::new(), None);
    }

    tag_buffer.push_str(raw_delta);

    let mut visible = String::new();
    let mut thinking = String::new();

    loop {
        if *in_think_block {
            if let Some(end_pos) = tag_buffer.find("</think>") {
                let think_part = &tag_buffer[..end_pos];
                if !think_part.is_empty() {
                    thinking.push_str(think_part);
                }
                *tag_buffer = tag_buffer[end_pos + 8..].to_string(); // "</think>"
                *in_think_block = false;
            } else {
                if !tag_buffer.is_empty() {
                    thinking.push_str(tag_buffer);
                    tag_buffer.clear();
                }
                break;
            }
        } else if let Some(start_pos) = tag_buffer.find("<think>") {
            let before = &tag_buffer[..start_pos];
            if !before.is_empty() {
                visible.push_str(before);
            }
            *tag_buffer = tag_buffer[start_pos + 7..].to_string(); // "<think>"
            *in_think_block = true;
        } else {
            if !tag_buffer.is_empty() {
                visible.push_str(tag_buffer);
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
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut in_think_block = false;
    let mut think_tag_buffer = String::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = chunk_result.map_err(|e| CoreError::Llm(format!("Stream read error: {e}")))?;
        let text = std::str::from_utf8(&chunk)
            .map_err(|e| CoreError::Llm(format!("Invalid UTF-8 in SSE stream: {e}")))?;
        buffer.push_str(text);

        // Process all complete lines currently in the buffer.
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            // Skip empty lines (SSE uses double-newline as event separator).
            if line.is_empty() {
                continue;
            }

            // Only process `data:` lines; ignore `event:`, `id:`, `retry:`, etc.
            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };

            let data = data.trim();

            // Stream termination signal.
            if data == "[DONE]" {
                // Flush trailing think content when provider did not close </think>.
                if in_think_block && !think_tag_buffer.is_empty() {
                    let tail = std::mem::take(&mut think_tag_buffer);
                    let _ = tx
                        .send(Ok(StreamChunk {
                            delta: String::new(),
                            tool_call_delta: None,
                            finish_reason: None,
                            usage: None,
                            thinking_delta: Some(tail),
                        }))
                        .await;
                }
                return Ok(());
            }

            // Parse JSON and send through channel.
            match serde_json::from_str::<SseChunk>(data) {
                Ok(sse) => {
                    let choice = sse.choices.as_ref().and_then(|c| c.first());
                    let raw_delta = choice
                        .map(extract_text_delta_from_choice)
                        .unwrap_or_default();
                    let (delta, think_from_tags) =
                        split_think_tags(&raw_delta, &mut in_think_block, &mut think_tag_buffer);
                    let finish_reason = choice
                        .and_then(|c| c.finish_reason.as_deref())
                        .map(parse_finish_reason);
                    let usage = sse.usage.map(|u| Usage {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                        thinking_tokens: u
                            .completion_tokens_details
                            .and_then(|d| d.reasoning_tokens),
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
                            return Ok(());
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
                                return Ok(());
                            }
                        }
                    }
                }
                Err(e) => {
                    // Send parse error through channel but continue processing.
                    let _ = tx
                        .send(Err(CoreError::Llm(format!("SSE JSON parse error: {e}"))))
                        .await;
                    continue;
                }
            }
        }
    }

    // Stream ended without [DONE] marker — server likely crashed or disconnected.
    Err(CoreError::StreamIncomplete)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
