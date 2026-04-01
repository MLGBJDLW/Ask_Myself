//! LLM-powered abstractive summarization of evicted conversation messages.
//!
//! When a conversation grows long enough that messages must be evicted from the
//! context window, this module can call the LLM to produce a concise summary
//! that retains key decisions, facts, and open items — rather than relying
//! solely on the extractive (truncation-based) recap.

use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, Role};
use tracing::warn;

use super::memory::estimate_tokens;

const SUMMARIZE_SYSTEM_PROMPT: &str = r#"You are a conversation summarizer. Given a section of conversation history, produce a concise summary that preserves:
1. Key decisions made
2. Important facts and data mentioned
3. Open questions or pending items
4. User preferences expressed
5. Tool results and their key findings

Be concise but complete. Output in the same language as the conversation."#;

/// Maximum tokens the summary LLM call may produce.
const MAX_SUMMARY_TOKENS: u32 = 500;

/// Maximum input characters sent into the summarisation request.
/// Keeps the summarisation call itself cheap (~2 000 tokens input).
const MAX_INPUT_FOR_SUMMARY: usize = 8_000;

/// Maximum number of retries for transient / rate-limited LLM errors.
const MAX_SUMMARY_RETRIES: u32 = 2;

/// Minimum estimated token count of the evicted text before it is worth
/// sending to the LLM (very short evictions are handled fine by the
/// extractive recap).
const MIN_TOKENS_FOR_LLM: u32 = 100;

/// Maximum chars kept per tool-result message to avoid blowing up input.
const TOOL_RESULT_CAP: usize = 500;

fn truncate_to_char_boundary(text: &str, max_len: usize) -> &str {
    if text.len() <= max_len {
        return text;
    }

    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Summarise evicted messages using an LLM.
///
/// Falls back to `extractive_fallback` if the LLM call fails or the evicted
/// content is too short to justify a round-trip.
pub async fn summarize_evicted_messages(
    provider: &dyn LlmProvider,
    model: &str,
    evicted_messages: &[Message],
    extractive_fallback: &str,
) -> String {
    let conversation_text = build_conversation_text(evicted_messages);

    if conversation_text.is_empty() || estimate_tokens(&conversation_text) < MIN_TOKENS_FOR_LLM {
        return extractive_fallback.to_string();
    }

    // Truncate input if it exceeds the budget.
    let input = if conversation_text.len() > MAX_INPUT_FOR_SUMMARY {
        format!(
            "{}...[earlier messages omitted]",
            truncate_to_char_boundary(&conversation_text, MAX_INPUT_FOR_SUMMARY)
        )
    } else {
        conversation_text
    };

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, SUMMARIZE_SYSTEM_PROMPT),
            Message::text(
                Role::User,
                format!("Summarize this conversation section:\n\n{}", input),
            ),
        ],
        max_tokens: Some(MAX_SUMMARY_TOKENS),
        temperature: Some(0.3),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
    };

    let mut retry_count = 0u32;
    loop {
        match provider.complete(&request).await {
            Ok(response) => {
                let text = response.content.trim();
                return if text.is_empty() {
                    extractive_fallback.to_string()
                } else {
                    text.to_string()
                };
            }
            Err(CoreError::RateLimited { retry_after_secs }) => {
                retry_count += 1;
                if retry_count > MAX_SUMMARY_RETRIES {
                    warn!(
                        "Summarizer: rate limited after {} retries, falling back to extractive recap",
                        MAX_SUMMARY_RETRIES
                    );
                    return extractive_fallback.to_string();
                }
                let wait = if retry_after_secs > 0 {
                    retry_after_secs
                } else {
                    2u64.pow(retry_count)
                };
                warn!(
                    "Summarizer: rate limited, retry {}/{} after {}s",
                    retry_count, MAX_SUMMARY_RETRIES, wait
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(CoreError::TransientLlm(msg)) => {
                retry_count += 1;
                if retry_count > MAX_SUMMARY_RETRIES {
                    warn!(
                        "Summarizer: transient error after {} retries: {}, falling back to extractive recap",
                        MAX_SUMMARY_RETRIES, msg
                    );
                    return extractive_fallback.to_string();
                }
                let wait = 2u64.pow(retry_count - 1); // 1s, 2s
                warn!(
                    "Summarizer: transient error (retry {}/{}): {}. Retrying after {}s",
                    retry_count, MAX_SUMMARY_RETRIES, msg, wait
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(e) => {
                // Non-retryable error (auth, bad request, etc.)
                warn!("Summarizer: non-retryable error: {e}, falling back to extractive recap");
                return extractive_fallback.to_string();
            }
        }
    }
}

/// Flatten a slice of [`Message`]s into a plain-text conversation transcript
/// suitable for feeding into the summariser prompt.
fn build_conversation_text(messages: &[Message]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    parts.push(format!("User: {}", text));
                }
            }
            Role::Assistant => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    parts.push(format!("Assistant: {}", text));
                }
            }
            Role::Tool => {
                let text = msg.text_content();
                if !text.trim().is_empty() {
                    let truncated = if text.len() > TOOL_RESULT_CAP {
                        truncate_to_char_boundary(&text, TOOL_RESULT_CAP)
                    } else {
                        text.as_str()
                    };
                    parts.push(format!("Tool result: {}", truncated));
                }
            }
            _ => {}
        }
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_conversation_text_basic() {
        let msgs = vec![
            Message::text(Role::User, "Hello"),
            Message::text(Role::Assistant, "Hi there!"),
        ];
        let text = build_conversation_text(&msgs);
        assert!(text.contains("User: Hello"));
        assert!(text.contains("Assistant: Hi there!"));
    }

    #[test]
    fn test_build_conversation_text_truncates_tool() {
        let long_tool = "x".repeat(1000);
        let msgs = vec![Message::text(Role::Tool, &long_tool)];
        let text = build_conversation_text(&msgs);
        // Tool result should be capped at TOOL_RESULT_CAP chars.
        let prefix = format!("Tool result: {}", &long_tool[..TOOL_RESULT_CAP]);
        assert!(text.starts_with(&prefix));
        assert!(text.len() < long_tool.len());
    }

    #[test]
    fn test_build_conversation_text_skips_empty() {
        let msgs = vec![
            Message::text(Role::User, ""),
            Message::text(Role::Assistant, "  "),
            Message::text(Role::User, "Actual content"),
        ];
        let text = build_conversation_text(&msgs);
        assert_eq!(text, "User: Actual content");
    }
}
