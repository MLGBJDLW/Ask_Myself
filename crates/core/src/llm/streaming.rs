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
    delta: SseDelta,
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct SseDelta {
    content: Option<String>,
    tool_calls: Option<Vec<SseToolCallDelta>>,
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
                return Ok(());
            }

            // Parse JSON and send through channel.
            match serde_json::from_str::<SseChunk>(data) {
                Ok(sse) => {
                    let choice = sse.choices.as_ref().and_then(|c| c.first());
                    let delta = choice
                        .and_then(|c| c.delta.content.clone())
                        .unwrap_or_default();
                    let finish_reason = choice
                        .and_then(|c| c.finish_reason.as_deref())
                        .map(parse_finish_reason);
                    let usage = sse.usage.map(|u| Usage {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                    });

                    // Emit text/finish/usage metadata as one chunk.
                    if !delta.is_empty() || finish_reason.is_some() || usage.is_some() {
                        if tx
                            .send(Ok(StreamChunk {
                                delta,
                                tool_call_delta: None,
                                finish_reason,
                                usage,
                            }))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                    }

                    // Emit each tool call delta separately so multiple tool calls
                    // in one SSE frame are preserved.
                    if let Some(tool_calls) = choice.and_then(|c| c.delta.tool_calls.as_ref()) {
                        for tc in tool_calls {
                            if tx
                                .send(Ok(StreamChunk {
                                    delta: String::new(),
                                    tool_call_delta: Some(map_tool_call_delta(tc)),
                                    finish_reason: None,
                                    usage: None,
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

    Ok(())
}
