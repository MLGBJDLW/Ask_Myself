use futures::stream::{self, BoxStream};

use crate::error::CoreError;
use crate::llm::{CompletionResponse, StreamChunk, ToolCallDelta};

const DISABLE_LLM_STREAMING_ENV: &str = "NEXA_DISABLE_LLM_STREAMING";
const LEGACY_DISABLE_LLM_STREAMING_ENV: &str = "ASK_MYSELF_DISABLE_LLM_STREAMING";

fn env_flag_enabled(name: &str) -> bool {
    let Ok(value) = std::env::var(name) else {
        return false;
    };
    let normalized = value.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "0" | "false" | "off" | "no" | "disabled"
        )
}

pub(super) fn llm_streaming_disabled_by_env() -> bool {
    env_flag_enabled(DISABLE_LLM_STREAMING_ENV)
        || env_flag_enabled(LEGACY_DISABLE_LLM_STREAMING_ENV)
}

pub(super) fn completion_response_to_agent_stream(
    response: CompletionResponse,
) -> BoxStream<'static, Result<StreamChunk, CoreError>> {
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
            tool_call_delta: Some(ToolCallDelta {
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

    Box::pin(stream::iter(chunks))
}
